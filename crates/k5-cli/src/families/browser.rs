//! `k5 browser note …` and `k5 browser redeliver …`: a page note filed
//! into a workspace's Thread as a Design tab's Send files it, as the shell
//! CLI's `cmd_browser` answers it (`cli/src/browser.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, a re-minted passport taken at once, and the two gates
//!    ([`crate::prelude`]); a line with `-h` or `--help` in it is offline,
//!    so only the connection gate stops it.
//! 2. `browser` is the `browser-open` tool, an `id` tool: with `HOOK_SOCK`
//!    set, the stale-passport or require-passport lines, exit 3.
//! 3. The verb: none, `-h`, `--help` or `help` is the help page; `note` and
//!    `redeliver` read their words ([`parse_note`], [`parse_redeliver`]);
//!    anything else is `Error: unknown browser subcommand '<it>'` and the
//!    help on stderr, exit 2. A value flag as the last word (`shift 2` with
//!    one word left, under `set -e`) ends the command with exit 1 and
//!    nothing printed, as the shell does.
//! 4. `raw="$(cli_send cli_post_json <route> <body>)"`: the cell socket
//!    first, then TCP (a POST again only after a refused connect or "not
//!    served"). A call that failed ends the command with its code; no
//!    answer is the transport's line and exit 1.
//! 5. `_browser_notes_reply` on what `$(…)` held ([`reply`]).
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).

use super::{curl_port, get, py_get, raised, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::{Call, Error};
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["browser"];
/// The binary answers it; [`run`] hands the edge cases back.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "browser-open";
pub const NOTES: &str = "/cli/browser/notes";
pub const REDELIVER: &str = "/cli/browser/notes/redeliver";

/// How the words end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// The help page on stdout, exit 0.
    Help,
    /// Lines on stderr; `help`: the help page after them, on stderr too.
    Stop {
        lines: Vec<String>,
        help: bool,
        exit: Exit,
    },
    /// A request: its route, its body (`json.dumps`), and `--json`.
    Send {
        route: &'static str,
        body: String,
        json: bool,
    },
}

fn stop(line: String, help: bool) -> Ask {
    Ask::Stop {
        lines: vec![line],
        help,
        exit: Exit::Usage,
    }
}

/// `shift 2` with one word left, under `set -e`: exit 1, nothing said.
fn short_shift() -> Ask {
    Ask::Stop {
        lines: Vec::new(),
        help: false,
        exit: Exit::Error,
    }
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("a body from words always prints")
}

/// `cmd_browser_note`'s words. `project`: `$PROJECT`; `readable`:
/// `[ -r "$f" ]`; `html`: what Python reads of the file.
pub fn parse_note(
    words: &[&str],
    project: &str,
    readable: impl Fn(&str) -> bool,
    html: impl Fn(&str) -> Result<String, Raise>,
) -> Ask {
    let (mut workspace, mut url, mut selector, mut html_file) = ("", "", "", "");
    let (mut text, mut title, mut intent, mut as_json) = ("", "", "change", false);
    let mut comment = "";
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let slot = match w {
            "--workspace" | "-w" => Some(&mut workspace),
            "--url" => Some(&mut url),
            "--selector" => Some(&mut selector),
            "--html-file" => Some(&mut html_file),
            "--text" => Some(&mut text),
            "--title" => Some(&mut title),
            "--intent" => Some(&mut intent),
            _ => None,
        };
        if let Some(slot) = slot {
            let Some(v) = words.get(i + 1) else {
                return short_shift();
            };
            *slot = *v;
            i += 2;
            continue;
        }
        match w {
            "--json" => as_json = true,
            "-h" | "--help" => return Ask::Help,
            _ if w.starts_with("--") => {
                return stop(format!("Error: unknown flag '{w}'"), true);
            }
            _ => {
                if !comment.is_empty() {
                    return stop("Error: one note text only (quote it)".into(), false);
                }
                comment = w;
            }
        }
        i += 1;
    }
    if url.is_empty() || comment.is_empty() {
        return stop("Error: --url and the note text are required".into(), true);
    }
    if !matches!(intent, "change" | "question") {
        return stop(
            format!("Error: --intent is change or question, got: {intent}"),
            false,
        );
    }
    if !html_file.is_empty() && !readable(html_file) {
        return stop(format!("Error: cannot read --html-file {html_file}"), false);
    }
    let ws = if workspace.is_empty() {
        project
    } else {
        workspace
    };
    let mut note = vec![
        (Text::from("comment"), s(comment)),
        (Text::from("intent"), s(intent)),
    ];
    if !selector.is_empty() || !html_file.is_empty() || !text.is_empty() {
        let html = if html_file.is_empty() {
            String::new()
        } else {
            match html(html_file) {
                Ok(h) => h,
                Err(r) => {
                    // The traceback, then `|| exit 2`.
                    return Ask::Stop {
                        lines: vec![r.text().trim_end_matches('\n').to_string()],
                        help: false,
                        exit: Exit::Usage,
                    };
                }
            }
        };
        note.push((
            Text::from("payload"),
            Value::Object(vec![
                (Text::from("selector"), s(selector)),
                (Text::from("html"), s(&html)),
                (Text::from("text"), s(text)),
            ]),
        ));
    }
    let body = Value::Object(vec![
        (Text::from("workspace"), s(ws)),
        (
            Text::from("page"),
            Value::Object(vec![
                (Text::from("url"), s(url)),
                (Text::from("title"), s(title)),
            ]),
        ),
        (Text::from("notes"), Value::Array(vec![Value::Object(note)])),
    ]);
    Ask::Send {
        route: NOTES,
        body: dumps(&body),
        json: as_json,
    }
}

/// `cmd_browser_redeliver`'s words.
pub fn parse_redeliver(words: &[&str], project: &str) -> Ask {
    let (mut workspace, mut item, mut as_json) = ("", "", false);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--workspace" | "-w" => {
                let Some(v) = words.get(i + 1) else {
                    return short_shift();
                };
                workspace = *v;
                i += 2;
                continue;
            }
            "--json" => as_json = true,
            "-h" | "--help" => return Ask::Help,
            _ if w.starts_with("--") => return stop(format!("Error: unknown flag '{w}'"), false),
            _ => item = w,
        }
        i += 1;
    }
    if item.is_empty() {
        return stop("Error: the Thread item id is required".into(), true);
    }
    let ws = if workspace.is_empty() {
        project
    } else {
        workspace
    };
    let body = Value::Object(vec![
        (Text::from("workspace"), s(ws)),
        (Text::from("threadItemId"), s(item)),
    ]);
    Ask::Send {
        route: REDELIVER,
        body: dumps(&body),
        json: as_json,
    }
}

/// `cmd_browser`'s words after `browser`.
pub fn parse(
    words: &[&str],
    project: &str,
    readable: impl Fn(&str) -> bool,
    html: impl Fn(&str) -> Result<String, Raise>,
) -> Ask {
    match words.first() {
        Some(&"note") => parse_note(&words[1..], project, readable, html),
        Some(&"redeliver") => parse_redeliver(&words[1..], project),
        None | Some(&("" | "-h" | "--help" | "help")) => Ask::Help,
        Some(other) => stop(format!("Error: unknown browser subcommand '{other}'"), true),
    }
}

/// Python's `open(f, encoding="utf-8", errors="replace").read(64 * 1024)`:
/// at most 65536 characters, `\r\n` and `\r` read as `\n`.
pub fn read_html(path: &str) -> Result<String, Raise> {
    let bytes = std::fs::read(path).map_err(|e| {
        let f = crate::out::pylib::json::os_error(&e, path);
        Raise::at(7, format!("{}: {}", f.class, f.message))
    })?;
    let text = String::from_utf8_lossy(&bytes)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    Ok(text.chars().take(64 * 1024).collect())
}

/// `[ -r "$f" ]`.
#[cfg(unix)]
pub fn readable(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::R_OK) == 0 }
}

#[cfg(not(unix))]
pub fn readable(path: &str) -> bool {
    std::fs::File::open(path).is_ok()
}

/// The reply's lines that raise.
const L_OK: u32 = 10;

/// `_browser_notes_reply <json> <raw>`.
pub fn reply(raw: &[u8], as_json: bool) -> Shown {
    let raw = String::from_utf8_lossy(raw).into_owned();
    let Ok(d) = json::parse(&raw) else {
        let head: String = raw.chars().take(200).collect();
        return Shown::err(
            format!(
                "Error: daemon returned an unparseable response: {}\n",
                py::repr(&s(&head))
            ),
            Exit::Error,
        );
    };
    match reply_of(&d, &raw, as_json) {
        Ok(shown) => shown,
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

fn reply_of(d: &Value, raw: &str, as_json: bool) -> Result<Shown, Raise> {
    let err = match d {
        Value::Object(p) => get(p, "error").cloned(),
        _ => None,
    };
    let err_true = err.as_ref().is_some_and(py::truthy);
    let failed = err_true || !py_get(d, "ok", L_OK)?.is_some_and(py::truthy);
    if failed {
        let err = match err {
            Some(Value::Object(e)) => {
                let pick = ["hint", "code"]
                    .iter()
                    .filter_map(|k| get(&e, k))
                    .find(|x| py::truthy(x))
                    .cloned();
                Some(pick.unwrap_or(Value::Object(e)))
            }
            other => other,
        };
        let msg = match py_get(d, "message", L_OK)?.filter(|m| py::truthy(m)) {
            Some(m) => py::str_of(m),
            None => match err.filter(py::truthy) {
                Some(e) => py::str_of(&e),
                None => raw.chars().take(200).collect(),
            },
        };
        return Ok(Shown::err(format!("Error: {msg}\n"), Exit::Error));
    }
    if as_json {
        return Ok(Shown::out(
            json::to_string(d, &Options::default()).expect("ASCII JSON always prints") + "\n",
        ));
    }
    let field = |k: &str| match d {
        Value::Object(p) => get(p, k).cloned(),
        _ => None,
    };
    let shown = |k: &str| field(k).map_or_else(|| "None".to_string(), |v| py::str_of(&v));
    let (addr, item) = (shown("addr"), shown("threadItemId"));
    if field("delivered").is_some_and(|x| py::truthy(&x)) {
        return Ok(Shown::out(format!(
            "Sent to {addr} (Thread item {item}).\n"
        )));
    }
    Ok(Shown::out(format!(
        concat!(
            "Posted to the {} Thread (item {}) but not delivered to the agent yet.\n",
            "Retry: ",
            k2_client::cli_name!(),
            " browser redeliver {} --workspace {}\n"
        ),
        addr, item, item, addr
    )))
}

/// `k5 browser …`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
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
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, &words, hook_sock.as_deref(), settings))
}

fn help_page() -> &'static str {
    crate::text::help("browser").expect("the browser help page is embedded")
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    let offline = words.iter().any(|w| matches!(*w, "-h" | "--help"));
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if offline => return Gate::CannotConnect.stop(),
        Some(gate) => return gate.stop(),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let (route, body, as_json) = match parse(words, &settings.project, readable, read_html) {
        Ask::Help => {
            return match out::stdout(help_page()) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Ask::Stop { lines, help, exit } => {
            for l in &lines {
                out::stderr_line(l);
            }
            if help {
                let _ = out::write_flushed(&mut std::io::stderr().lock(), help_page().as_bytes());
            }
            return exit;
        }
        Ask::Send { route, body, json } => (route, body, json),
    };
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
    let exit = match client.send(route, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            reply(&super::connections::held(&r.body), as_json).print()
        }
        Err(
            e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. }),
        ) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(&mut client, exit);
    exit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(words: &[&str]) -> Ask {
        parse(words, "/p", |_| true, |_| Ok("<b>x</b>".into()))
    }

    #[test]
    fn the_bodies() {
        assert_eq!(
            note(&["note", "--url", "http://x", "fix é", "--json"]),
            Ask::Send {
                route: NOTES,
                body: r#"{"workspace": "/p", "page": {"url": "http://x", "title": ""}, "notes": [{"comment": "fix \u00e9", "intent": "change"}]}"#.into(),
                json: true
            }
        );
        assert_eq!(
            note(&["note", "c", "--url", "u", "--html-file", "f", "-w", "ws", "--intent", "question"]),
            Ask::Send {
                route: NOTES,
                body: r#"{"workspace": "ws", "page": {"url": "u", "title": ""}, "notes": [{"comment": "c", "intent": "question", "payload": {"selector": "", "html": "<b>x</b>", "text": ""}}]}"#.into(),
                json: false
            }
        );
        assert_eq!(
            note(&["redeliver", "t-1", "t-2"]),
            Ask::Send {
                route: REDELIVER,
                body: r#"{"workspace": "/p", "threadItemId": "t-2"}"#.into(),
                json: false
            }
        );
    }

    #[test]
    fn the_stops() {
        assert_eq!(note(&[]), Ask::Help);
        assert_eq!(note(&["note", "--url", "u", "x", "--help"]), Ask::Help);
        assert_eq!(note(&["note", "--url"]), short_shift());
        assert_eq!(
            note(&["zap"]),
            stop("Error: unknown browser subcommand 'zap'".into(), true)
        );
        assert_eq!(
            note(&["note", "a", "b"]),
            stop("Error: one note text only (quote it)".into(), false)
        );
        assert_eq!(
            note(&["note", "--url", "u"]),
            stop("Error: --url and the note text are required".into(), true)
        );
        assert_eq!(
            note(&["note", "--zz"]),
            stop("Error: unknown flag '--zz'".into(), true)
        );
        assert_eq!(
            note(&["redeliver", "--zz"]),
            stop("Error: unknown flag '--zz'".into(), false)
        );
        assert_eq!(
            note(&["note", "--url", "u", "x", "--intent", "y"]),
            stop(
                "Error: --intent is change or question, got: y".into(),
                false
            )
        );
    }

    #[test]
    fn the_replies() {
        let r = reply(
            br#"{"ok":true,"delivered":true,"addr":"ws","threadItemId":"t1"}"#,
            false,
        );
        assert_eq!(r.stdout, "Sent to ws (Thread item t1).\n");
        let r = reply(br#"{"ok":true,"addr":"ws","threadItemId":"t1"}"#, false);
        assert!(r.stdout.ends_with(" browser redeliver t1 --workspace ws\n"));
        let r = reply(br#"{"ok":false,"error":{"code":"not_yours"}}"#, false);
        assert_eq!(
            (r.stderr.as_str(), r.exit),
            ("Error: not_yours\n", Exit::Error)
        );
        let r = reply(b"nope", false);
        assert_eq!(
            r.stderr,
            "Error: daemon returned an unparseable response: 'nope'\n"
        );
        let r = reply(b"[1]", false);
        assert!(r
            .stderr
            .ends_with("AttributeError: 'list' object has no attribute 'get'\n"));
    }
}
