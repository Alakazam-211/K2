//! The verbs on one named workspace (`cli/src/talk-and-workspace.sh`):
//!
//! - `cleanup`: `cli_request_post /cli/workspace/cleanup`, its answer as
//!   it came (any words after it are not read).
//! - `resume-chat-args <ws>`: the lookup ([`super::Sent::resolve`]), then
//!   `cli_request /cli/workspace/resume-chat-args`, its answer as it came.
//! - `agent-name <ws>` and `set-agent-name <ws> <name>`: the lookup, then
//!   the request into a reader that prints the daemon's `error` (exit 1)
//!   or the name. Any exception is `Error: <it>`, exit 1.
//! - `set-handle <ws> <handle> --i-know-this-breaks-federation`: without
//!   the flag, four lines and exit 1; else the lookup, then `cli_post_json
//!   /cli/workspace/set-handle` into its reader.
//! - `api-key <ws> [--key-stdin]`: the key from stdin (a hidden prompt on
//!   a terminal, else one line) or, deprecated, the second word (with its
//!   warning); none clears it. The lookup, then the key into the body and
//!   `cli_post_json_stdin /cli/workspace/api-key` into its reader. The key
//!   is never on any argv or in any environment, and never printed.
//! - `swap-canonical <ws> --agent <harness> [--recent|--full]
//!   [--mode m] [--notes-file f] [--model id]`: in a session the passport
//!   is required (else `_cli_require_session_passport`, exit 3) and sent;
//!   the lookup (a failed one goes on with the name as typed); the body
//!   (the notes file as Python's `open(…, errors="replace")` reads it);
//!   one POST of it over TCP, at most 120 s, sent once and not healed, no
//!   line of its own: any failure is `Error: could not reach the daemon`,
//!   exit 1. Then its reader.
//!
//! Usage lines are exit 1 (`swap-canonical`'s, 2). A notes file that
//! can't be read is the shell's.

use super::{
    decode_error, dumps, error_text, obj, py_get, py_in, py_index, repr_head, stop, Ask as Top,
    Here, Py, Sent,
};
use crate::exit::Exit;
use crate::families::{pipefail, s, Piped, Shown};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::prelude;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel};
use k2_client::cli_name;
use std::time::Duration;

pub const CLEANUP: &str = "/cli/workspace/cleanup";
pub const RESUME: &str = "/cli/workspace/resume-chat-args";
pub const NAME: &str = "/cli/workspace/agent-display-name";
pub const SET_NAME: &str = "/cli/workspace/set-agent-display-name";
pub const SET_HANDLE: &str = "/cli/workspace/set-handle";
pub const API_KEY: &str = "/cli/workspace/api-key";
pub const SWAP: &str = "/cli/workspace/swap-canonical";

const SWAP_USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace swap-canonical <workspace> --agent <harness> [--recent|--full] [--notes-file <path>] [--model <id>]"
);

/// The confirm flag `set-handle` needs.
pub const CONFIRM: &str = "--i-know-this-breaks-federation";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Cleanup,
    Resume(String),
    Name(String),
    SetName(String, String),
    SetHandle(String, String),
    ApiKey(ApiKey),
    Swap(Swap),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub target: String,
    /// `--key-stdin`: the key is read when the command runs.
    pub from_stdin: bool,
    /// The deprecated key on the command line (empty: clear the key).
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Swap {
    pub target: String,
    pub agent: String,
    pub mode: String,
    pub notes: String,
    pub model: String,
}

fn usage1(line: &str) -> Top {
    stop(&[line], Exit::Error)
}

/// The words after the verb.
pub fn parse(verb: &str, words: &[&str], _here: &Here) -> Top {
    let arg = |i: usize| words.get(i).copied().unwrap_or("").to_string();
    let named = |a: Ask| Top::Named(a);
    match verb {
        "cleanup" => named(Ask::Cleanup),
        "resume-chat-args" if arg(0).is_empty() => usage1(concat!(
            "Usage: ",
            cli_name!(),
            " workspace resume-chat-args <workspace>"
        )),
        "resume-chat-args" => named(Ask::Resume(arg(0))),
        "agent-name" if arg(0).is_empty() => usage1(concat!(
            "Usage: ",
            cli_name!(),
            " workspace agent-name <workspace>"
        )),
        "agent-name" => named(Ask::Name(arg(0))),
        "set-agent-name" if arg(0).is_empty() || arg(1).is_empty() => usage1(concat!(
            "Usage: ",
            cli_name!(),
            " workspace set-agent-name <workspace> <new-name>"
        )),
        "set-agent-name" => named(Ask::SetName(arg(0), arg(1))),
        "set-handle" => set_handle(words),
        "api-key" => api_key(words),
        _ => swap(words),
    }
}

fn set_handle(words: &[&str]) -> Top {
    let confirm = words.contains(&CONFIRM);
    let args: Vec<&str> = words.iter().copied().filter(|w| *w != CONFIRM).collect();
    let target = args.first().copied().unwrap_or("");
    let handle = args.get(1).copied().unwrap_or("");
    if target.is_empty() || handle.is_empty() {
        return usage1(concat!(
            "Usage: ",
            cli_name!(),
            " workspace set-handle <workspace> <handle> --i-know-this-breaks-federation"
        ));
    }
    if !confirm {
        return stop(
            &[
                "Changing the handle changes this agent's address (old::host → new::host).",
                "Existing federated connections will break until the other side reconnects or updates the handle.",
                "The old handle stays with this workspace (never given to another one).",
                "Re-run with --i-know-this-breaks-federation to confirm.",
            ],
            Exit::Error,
        );
    }
    Top::Named(Ask::SetHandle(target.to_string(), handle.to_string()))
}

const KEY_USAGE: [&str; 2] = [
    concat!(
        "Usage: ",
        cli_name!(),
        " workspace api-key <workspace> --key-stdin   (key read from stdin; never an argument)"
    ),
    concat!(
        "       ",
        cli_name!(),
        " workspace api-key <workspace>               (no key: clears the stored key)"
    ),
];

fn api_key(words: &[&str]) -> Top {
    let mut from_stdin = false;
    let mut args: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "--key-stdin" => from_stdin = true,
            // The router took any `-h` or `--help` before.
            "--" => {
                args.extend(&words[i + 1..]);
                break;
            }
            w => args.push(w),
        }
        i += 1;
    }
    let target = args.first().copied().unwrap_or("");
    if target.is_empty() || args.len() > 2 {
        return stop(&KEY_USAGE, Exit::Error);
    }
    if from_stdin && args.len() > 1 {
        return stop(
            &["Error: give the key on stdin (--key-stdin) or not at all, not both."],
            Exit::Usage,
        );
    }
    Top::Named(Ask::ApiKey(ApiKey {
        target: target.to_string(),
        from_stdin,
        key: args.get(1).copied().unwrap_or("").to_string(),
    }))
}

fn swap(words: &[&str]) -> Top {
    let (mut target, mut agent, mut mode, mut notes, mut model) = (
        String::new(),
        String::new(),
        "recent".to_string(),
        String::new(),
        String::new(),
    );
    let usage = || stop(&[SWAP_USAGE], Exit::Usage);
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "--model" | "--agent" | "--provider" | "--mode" | "--notes-file" => {
                let v = words.get(i + 1).copied().unwrap_or("");
                if v.is_empty() {
                    return usage();
                }
                let v = v.to_string();
                match w {
                    "--model" => model = v,
                    "--mode" => mode = v,
                    "--notes-file" => notes = v,
                    _ => agent = v,
                }
                i += 2;
                continue;
            }
            "--recent" => mode = "recent".into(),
            "--full" => mode = "full".into(),
            w if w.starts_with('-') => {
                return stop(&[&format!("Unknown flag: {w}"), SWAP_USAGE], Exit::Usage)
            }
            w => {
                if target.is_empty() {
                    target = w.to_string();
                } else {
                    return stop(&[&format!("Unexpected argument: {w}")], Exit::Usage);
                }
            }
        }
        i += 1;
    }
    if target.is_empty() || agent.is_empty() {
        return usage();
    }
    if !matches!(mode.as_str(), "recent" | "full") {
        return stop(&["mode must be recent or full"], Exit::Usage);
    }
    let mut text = String::new();
    if !notes.is_empty() {
        if !std::path::Path::new(&notes).is_file() {
            return stop(&[&format!("Error: file not found: {notes}")], Exit::Error);
        }
        let Ok(bytes) = std::fs::read(&notes) else {
            return Top::Shell;
        };
        // `open(path, encoding="utf-8", errors="replace").read()`: bad
        // bytes are U+FFFD, and every line end is `\n`.
        text = String::from_utf8_lossy(&bytes)
            .replace("\r\n", "\n")
            .replace('\r', "\n");
    }
    Top::Named(Ask::Swap(Swap {
        target,
        agent,
        mode,
        notes: text,
        model,
    }))
}

/// The verb's requests and output.
pub fn send(w: &mut Sent, a: &Ask) -> Exit {
    match a {
        Ask::Cleanup => as_it_came(w, CLEANUP, true, &[]),
        Ask::Resume(target) => {
            let path = match w.resolve(target) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let p = format!("project={}", quote(&path));
            as_it_came(w, RESUME, false, &[&p])
        }
        Ask::Name(target) => {
            let path = match w.resolve(target) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let p = format!("project={}", quote(&path));
            let r = w.request(NAME, &[&p]);
            let piped = w.piped(r);
            reader(&piped, |d| {
                Ok(py::str_of(py_get(d, "display_name")?.unwrap_or(&s(""))))
            })
        }
        Ask::SetName(target, name) => {
            let path = match w.resolve(target) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let p = format!("project={}&name={}", quote(&path), quote(name));
            let r = w.request_post(SET_NAME, &[&p]);
            let piped = w.piped(r);
            reader(&piped, |d| {
                Ok(format!(
                    "Set agent display name to: {}",
                    py::str_of(py_get(d, "display_name")?.unwrap_or(&s("")))
                ))
            })
        }
        Ask::SetHandle(target, handle) => {
            let path = match w.resolve(target) {
                Ok(p) => p,
                Err(exit) => return exit,
            };
            let body = dumps(&obj(&[("project", s(&path)), ("handle", s(handle))]));
            let r = w.post_json(SET_HANDLE, &body);
            let piped = w.piped(r);
            handle_reader(&piped)
        }
        Ask::ApiKey(k) => api_key_send(w, k),
        Ask::Swap(sw) => swap_send(w, sw),
    }
}

/// `cli_request[_post] <endpoint> <args>` at the top: the answer as it
/// came (its NULs dropped), and curl's code; no answer is the line, then
/// exit 1.
fn as_it_came(w: &mut Sent, endpoint: &str, post: bool, args: &[&str]) -> Exit {
    let r = if post {
        w.request_post(endpoint, args)
    } else {
        w.request(endpoint, args)
    };
    match r {
        Ok(r) => {
            w.note(&r);
            let body = crate::families::without_nuls(&r.body);
            match out::write_flushed(&mut std::io::stdout().lock(), &body) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(e) => w.failed(&e, false),
    }
}

/// The readers of `agent-name`, `set-agent-name` and `api-key`:
/// `json.load`, then `"error" in d` (its `d["error"]` on stderr, exit 1),
/// else `ok`'s line; any exception is `Error: <it>`, exit 1.
fn reader(piped: &Piped, ok: impl Fn(&Value) -> Result<String, String>) -> Exit {
    let raw = String::from_utf8_lossy(&piped.body).into_owned();
    let run = || -> Result<Result<String, String>, String> {
        let d = json::parse(&raw).map_err(|_| decode_error(&raw))?;
        if py_in("error", &d)? {
            return Ok(Err(py::str_of(py_index(&d, "error")?)));
        }
        Ok(Ok(ok(&d)?))
    };
    let shown = match run() {
        Ok(Ok(line)) => Shown::out(format!("{line}\n")),
        Ok(Err(e)) | Err(e) => Shown::err(format!("Error: {e}\n"), Exit::Error),
    };
    pipefail(piped.exit, shown.print())
}

/// `set-handle`'s reader: as [`reader`], the error a dict's `hint` or
/// `code`.
fn handle_reader(piped: &Piped) -> Exit {
    let raw = String::from_utf8_lossy(&piped.body).into_owned();
    let run = || -> Result<Result<String, String>, String> {
        let d = json::parse(&raw).map_err(|_| decode_error(&raw))?;
        if py_in("error", &d)? {
            let err = py_get(&d, "error")?.cloned().unwrap_or(Value::Null);
            return Ok(Err(error_text(&err)));
        }
        Ok(Ok(format!(
            "Set handle to: {}",
            py::str_of(py_get(&d, "handle")?.unwrap_or(&s("")))
        )))
    };
    let shown = match run() {
        Ok(Ok(line)) => Shown::out(format!("{line}\n")),
        Ok(Err(e)) | Err(e) => Shown::err(format!("Error: {e}\n"), Exit::Error),
    };
    pipefail(piped.exit, shown.print())
}

/// `_cli_read_secret_stdin`: a hidden prompt when stdin is a terminal
/// (`read -rs`; nothing when it ends before a line end), else one line of
/// the pipe. `read` drops NULs.
fn read_secret(prompt: &str) -> String {
    use std::io::Read;
    let tty = super::is_tty(0);
    let mut line = Vec::new();
    let mut ended = false;
    let hidden = tty.then(|| {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), prompt.as_bytes());
        Echo::off()
    });
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut b = [0u8; 1];
    loop {
        match lock.read(&mut b) {
            Ok(1) if b[0] == b'\n' => {
                ended = true;
                break;
            }
            Ok(1) => {
                if b[0] != 0 {
                    line.push(b[0]);
                }
            }
            _ => break,
        }
    }
    if let Some(echo) = hidden {
        drop(echo);
        let _ = out::write_flushed(&mut std::io::stderr().lock(), b"\n");
        if !ended {
            line.clear();
        }
    }
    String::from_utf8_lossy(&line).into_owned()
}

/// The terminal's echo, off until dropped.
struct Echo {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl Echo {
    #[cfg(unix)]
    fn off() -> Echo {
        // SAFETY: tcgetattr and tcsetattr on stdin with a zeroed termios
        // they fill; the saved state is put back on drop.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return Echo { saved: None };
            }
            let saved = t;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            Echo { saved: Some(saved) }
        }
    }

    #[cfg(not(unix))]
    fn off() -> Echo {
        Echo {}
    }
}

impl Drop for Echo {
    #[cfg(unix)]
    fn drop(&mut self) {
        if let Some(t) = &self.saved {
            // SAFETY: puts back the state tcgetattr read.
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, t);
            }
        }
    }

    #[cfg(not(unix))]
    fn drop(&mut self) {}
}

fn api_key_send(w: &mut Sent, k: &ApiKey) -> Exit {
    let key = if k.from_stdin {
        let key = read_secret("Paste the API key: ");
        if key.is_empty() {
            out::stderr_line("Error: no key on stdin (to clear the key, run without --key-stdin and without a key).");
            return Exit::Usage;
        }
        key
    } else {
        if !k.key.is_empty() {
            out::stderr_line("warning: passing a key on the command line is deprecated and will be refused in the next release; use --key-stdin");
        }
        k.key.clone()
    };
    let path = match w.resolve(&k.target) {
        Ok(p) => p,
        Err(exit) => return exit,
    };
    let body = dumps(&obj(&[
        ("project", s(&path)),
        ("key", s(super::py_strip(&key))),
    ])) + "\n";
    let r = w.client.post_json_stdin(API_KEY, body.as_bytes());
    let piped = match r {
        Ok(r) => {
            w.note(&r);
            Piped {
                body: crate::families::piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    };
    reader(&piped, |d| {
        let set = py_get(d, "keySet")?.is_some_and(py::truthy);
        Ok(format!(
            "Workspace API key {}.",
            if set { "set" } else { "cleared" }
        ))
    })
}

fn swap_send(w: &mut Sent, sw: &Swap) -> Exit {
    if w.settings.session && w.client.passport().is_none() {
        for l in prelude::require_passport_lines("workspace swap-canonical") {
            out::stderr_line(&l);
        }
        return Exit::Refused;
    }
    // `|| true`: a lookup that failed goes on with the name as typed;
    // one with no answer still ends the command.
    let q = format!("q={}", quote(&sw.target));
    let path = match w.request(super::RESOLVE, &[&q]) {
        Ok(r) => {
            w.note(&r);
            super::resolved_of(&crate::families::piped_body(&r))
        }
        Err(
            e @ (crate::transport::Error::NoAnswer(_)
            | crate::transport::Error::MaybeApplied { .. }),
        ) => return w.failed_in_pipe(&e, true),
        Err(_) => String::new(),
    };
    let path = if path.is_empty() {
        sw.target.clone()
    } else {
        path
    };
    let body = dumps(&obj(&[
        ("project", s(&path)),
        ("provider", s(&sw.agent)),
        ("mode", s(&sw.mode)),
        ("notes", s(&sw.notes)),
        ("model", s(&sw.model)),
    ])) + "\n";
    // `_K2_CURL_TRIES=1 _k2_curl` with the config's url, auth and
    // data-raw: cli_post_json's request, sent once, at most 120 s, with no
    // heal; `_permission_note "$out"` after it.
    w.client.post_max_time = Some(Duration::from_secs(120));
    w.client.healer = None;
    let r = match w
        .client
        .call(Channel::Tcp, SWAP, Call::PostJson(body.as_bytes()))
    {
        Ok(r) => r,
        Err(_) => {
            out::stderr_line("Error: could not reach the daemon");
            return Exit::Error;
        }
    };
    w.note(&r);
    let raw = String::from_utf8_lossy(&super::super::held(&r.body)).into_owned();
    swap_text(&raw).print()
}

/// Codes that refuse the caller (exit 3).
const REFUSED: [&str; 6] = [
    "harness_needs_person",
    "chat_refresh_needs_person",
    "chat_refresh_main_only",
    "other_workspace",
    "wrong_credential",
    "invalid_token",
];

/// The Python lines a traceback names.
pub mod line {
    pub const ERROR: u32 = 9;
    pub const SWAPPED: u32 = 20;
}

/// `swap-canonical`'s reader on what `$(…)` held.
pub fn swap_text(raw: &str) -> Shown {
    use crate::families::Raise;
    let mut p = Py::default();
    let d = match json::parse(raw) {
        Ok(d) => d,
        Err(_) => {
            p.eprint(&format!(
                "Error: daemon returned an unparseable response: {}",
                repr_head(raw, 300)
            ));
            return p.done(Exit::Error);
        }
    };
    let attr = |line: u32| move |e: String| Raise::at(line, format!("AttributeError: {e}"));
    let run = |p: &mut Py| -> Result<Exit, Raise> {
        let g = |k: &str, line: u32| -> Result<Value, Raise> {
            Ok(py_get(&d, k)
                .map_err(attr(line))?
                .cloned()
                .unwrap_or(Value::Null))
        };
        let err = g("error", line::ERROR)?;
        if py::truthy(&err) && !py::truthy(&g("swapped", line::ERROR)?) {
            let (code, shown) = match &err {
                Value::Object(_) => {
                    let code = py_get(&err, "code")
                        .ok()
                        .flatten()
                        .cloned()
                        .unwrap_or(Value::Null);
                    (code, error_text(&err))
                }
                e => (s(""), py::str_of(e)),
            };
            p.eprint(&format!("Error: {shown}"));
            let path = crate::families::remind::shared::or(Some(&g("handoffPath", 15)?), s(""));
            if py::truthy(&path) {
                p.eprint(&format!("Handoff file: {}", py::str_of(&path)));
            }
            let refused = REFUSED.iter().any(|c| super::is_str(&code, c));
            return Ok(if refused { Exit::Refused } else { Exit::Error });
        }
        if !py::truthy(&g("swapped", line::SWAPPED)?) {
            p.eprint("Error: canonical chat was not swapped");
            return Ok(Exit::Error);
        }
        let or_empty = |v: Value| crate::families::remind::shared::or(Some(&v), s(""));
        p.print(&format!(
            "Swapped the canonical chat to {}.",
            py::str_of(&or_empty(g("provider", 23)?))
        ));
        let model = g("model", 24)?;
        if py::truthy(&model) {
            p.print(&format!(
                "Model: {} (for this start; a later resume uses the preset or workspace model again)",
                py::str_of(&model)
            ));
        }
        for (key, label, line) in [
            ("fromProvider", "Previous harness", 26),
            ("sessionId", "Terminal session", 29),
            ("claudeSessionId", "New conversation", 32),
            ("handoffPath", "Handoff file", 35),
            ("seedError", "Transcript seed", 38),
        ] {
            let v = or_empty(g(key, line)?);
            if py::truthy(&v) {
                p.print(&format!("{label}: {}", py::str_of(&v)));
            }
        }
        if py::truthy(&g("messageDelivered", 41)?) {
            p.print("The handoff was sent into the new chat.");
        } else {
            let reason = crate::families::remind::shared::or(
                Some(&g("messageReason", 44)?),
                s("not delivered"),
            );
            let hint = or_empty(g("messageHint", 45)?);
            p.print(&format!(
                "The new chat is up. The handoff was not sent ({}).",
                py::str_of(&reason)
            ));
            if py::truthy(&hint) {
                p.print(&py::str_of(&hint));
            }
            p.print(
                "The handoff file is on disk. Paste it into the new chat if it did not arrive.",
            );
        }
        p.print("Heartbeats on the pinned chat follow it; heartbeats that resume their own session were not moved.");
        Ok(Exit::Ok)
    };
    match run(&mut p) {
        Ok(exit) => p.done(exit),
        Err(r) => p.raised(&r),
    }
}
