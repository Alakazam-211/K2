//! The `k5 sessions` verbs that change something, as the shell's
//! `cmd_sessions_open`, `cmd_sessions_new`, `cmd_sessions_stop` and
//! `cmd_sessions_main … set` (`cli/src/sessions.sh`) do them.
//!
//! After the gates ([`super`]), in the shell's order:
//!
//! 1. The verb's loop ([`parse`]): each mistake is one `usage:` line, exit
//!    2. A value flag as the last word is the shell's `_need_value` line,
//!    exit 2.
//! 2. The brief (`open`, `new`): `--brief <file>` read whole, `--brief -`
//!    all of stdin, or `--brief-text`; both is a `usage:` line. A missing
//!    file is a `usage:` line, a brief that is not UTF-8 is
//!    `brief_not_text`, each exit 2.
//! 3. The body, as the shell's Python writes it (`json.dumps`), then one
//!    `_sidecar_send POST` ([`crate::confirm::sidecar_call`]): the cell
//!    socket in a session, else TCP; a POST the socket may have run is
//!    its line and exit 1.
//! 4. `open` and `main set` are `_sessions_render`'s: a 404 that names no
//!    route is `daemon_too_old`, any other status but 200 `_sidecar_fail`'s
//!    line with its exit and the refusal's closing line; `--json` prints
//!    the body as it came; else the shell's Python lays it out
//!    ([`open_lines`], [`main_set_lines`]). `new` and `stop` are
//!    `_sidecar_render`'s ([`super::super::sidecar::lay_out`]).
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers `--brief -` when `mktemp` has no folder to write in.

use super::super::sidecar::{self, Reader};
use super::super::{py_get, raised, s, Raise, Shown};
use super::{exit_for, fail_line, TOO_OLD};
use crate::confirm::{self, Split};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::json as pyjson;
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::Client;
use k2_client::brand::CLI_NAME as CLI;
use std::io::Read;
use std::path::Path;

pub const OPEN: &str = "/cli/sessions/open";
pub const MAIN_SET: &str = "/cli/sessions/main/set";
pub const NEW: &str = "/cli/sidecar/new";
pub const STOP: &str = "/cli/sidecar/stop";

/// Where the brief comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Brief {
    pub file: String,
    pub text: String,
    pub have_text: bool,
}

/// A verb that changes something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Write {
    Open {
        id: String,
        name: String,
        workspace: String,
        harness: String,
        model: String,
        force: bool,
        brief: Brief,
        json: bool,
    },
    New {
        name: String,
        harness: String,
        workspace: String,
        model: String,
        brief: Brief,
        json: bool,
    },
    Stop(sidecar::Simple),
    MainSet {
        id: String,
        workspace: String,
        thread: String,
        json: bool,
    },
    /// `live` ([`super::live`]): a read, but the owner's raw view.
    Live {
        path: String,
        mode: super::live::Mode,
    },
}

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// One line on stderr (none when empty) and the exit; nothing sent.
    Stop(String, Exit),
    Do(Write),
}

/// `open` with a second id.
pub fn open_usage() -> String {
    format!("usage: {CLI} sessions open <id> [--name <name>] [--brief <file>|-] [--force]")
}

/// `new` with no name.
pub fn new_usage() -> String {
    format!(
        "usage: {CLI} sessions new <name> [--cli <cli>] [--brief <file>|-] [--brief-text \"\u{2026}\"]"
    )
}

/// `main set` with no id.
pub fn main_set_usage() -> String {
    format!("usage: {CLI} sessions main set <id>  (ids: {CLI} sessions --past)")
}

fn unknown_flag(w: &str) -> Plan {
    usage(format!("usage: unknown flag '{w}' ({CLI} sessions --help)"))
}

fn usage(line: String) -> Plan {
    Plan::Stop(line, Exit::Usage)
}

/// A value flag as the last word: the shell's `_need_value` line, exit 2.
fn shift(flag: &str) -> Plan {
    Plan::Stop(crate::out::need_value(flag), Exit::Usage)
}

/// The words after `sessions`, when they are a verb this module does.
/// `None`: not one of them (the module's caller reads them).
pub fn parse(words: &[&str]) -> Option<Plan> {
    let (sub, rest) = words.split_first()?;
    if rest.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return None;
    }
    match *sub {
        "open" => Some(open(rest)),
        "new" => Some(new(rest)),
        "stop" => {
            let owned: Vec<String> = rest.iter().map(|w| w.to_string()).collect();
            Some(match sidecar::parse_simple("stop", &owned) {
                Ok(n) => Plan::Do(Write::Stop(n)),
                Err(sidecar::Stop::Usage(line)) => usage(line),
                Err(sidecar::Stop::Shift(flag)) => shift(&flag),
            })
        }
        "main" if rest.contains(&"set") => main_set(rest),
        "live" => Some(match super::live::parse(rest) {
            (path, _) if path.is_empty() => {
                Plan::Stop(super::live::usage().trim_end().to_string(), Exit::Error)
            }
            (path, mode) => Plan::Do(Write::Live { path, mode }),
        }),
        _ => None,
    }
}

/// `--brief` and `--brief-text` both, or a file that is not there.
fn brief_check(b: &Brief) -> Option<Plan> {
    if !b.file.is_empty() && b.have_text {
        return Some(usage(
            "usage: pass --brief or --brief-text, not both".into(),
        ));
    }
    if !b.file.is_empty() && b.file != "-" && !Path::new(&b.file).is_file() {
        return Some(usage(format!("usage: brief file not found: {}", b.file)));
    }
    None
}

fn open(words: &[&str]) -> Plan {
    let (mut id, mut name, mut workspace, mut harness, mut model) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let (mut force, mut json) = (false, false);
    let mut brief = Brief::default();
    let mut i = 0;
    while i < words.len() {
        let a = words[i];
        let slot: Option<&mut String> = match a {
            "--name" => Some(&mut name),
            "--brief" => Some(&mut brief.file),
            "--brief-text" => {
                brief.have_text = true;
                Some(&mut brief.text)
            }
            "--workspace" => Some(&mut workspace),
            "--harness" => Some(&mut harness),
            "--model" => Some(&mut model),
            _ => None,
        };
        if let Some(slot) = slot {
            let Some(v) = words.get(i + 1) else {
                return shift(a);
            };
            *slot = v.to_string();
            i += 2;
            continue;
        }
        match a {
            "--force" => force = true,
            "--json" => json = true,
            _ if a.starts_with('-') => return unknown_flag(a),
            _ if id.is_empty() => id = a.to_string(),
            _ => return usage(open_usage()),
        }
        i += 1;
    }
    if id.is_empty() {
        return usage(format!("{}  (ids: {CLI} sessions --past)", open_usage()));
    }
    if let Some(stop) = brief_check(&brief) {
        return stop;
    }
    Plan::Do(Write::Open {
        id,
        name,
        workspace,
        harness,
        model,
        force,
        brief,
        json,
    })
}

fn new(words: &[&str]) -> Plan {
    let (mut name, mut cli, mut harness, mut workspace, mut model) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut json = false;
    let mut brief = Brief::default();
    let mut i = 0;
    while i < words.len() {
        let a = words[i];
        let slot: Option<&mut String> = match a {
            "--cli" => Some(&mut cli),
            "--harness" => Some(&mut harness),
            "--brief" => Some(&mut brief.file),
            "--brief-text" => {
                brief.have_text = true;
                Some(&mut brief.text)
            }
            "--workspace" => Some(&mut workspace),
            "--model" => Some(&mut model),
            _ => None,
        };
        if let Some(slot) = slot {
            let Some(v) = words.get(i + 1) else {
                return shift(a);
            };
            *slot = v.to_string();
            i += 2;
            continue;
        }
        match a {
            "--worktree" | "--base" => {
                return usage(format!(
                    "usage: {CLI} sessions new has no {a}. For a sidecar in its own git worktree: {CLI} sidecar new <name> --harness <preset> --worktree [--base <ref>] ({CLI} sidecar --help)"
                ))
            }
            "--json" => json = true,
            _ if a.starts_with('-') => return unknown_flag(a),
            _ if name.is_empty() => name = a.to_string(),
            _ => {
                return usage(format!(
                    "usage: one name only; quote names with spaces ({CLI} sessions --help)"
                ))
            }
        }
        i += 1;
    }
    if name.is_empty() {
        return usage(new_usage());
    }
    if !cli.is_empty() && !harness.is_empty() {
        return usage("usage: pass --cli or --harness, not both".into());
    }
    match cli.as_str() {
        "" => {}
        "cursor" => harness = "cursor-agent".into(),
        "claude" | "codex" | "grok" | "gemini" | "cursor-agent" | "pi" | "hermes" => {
            harness = cli.clone()
        }
        other => {
            return usage(format!(
                "usage: unknown --cli '{other}' (claude, codex, grok, gemini, cursor, pi, hermes)"
            ))
        }
    }
    if let Some(stop) = brief_check(&brief) {
        return stop;
    }
    Plan::Do(Write::New {
        name,
        harness,
        workspace,
        model,
        brief,
        json,
    })
}

fn main_set(words: &[&str]) -> Option<Plan> {
    let (mut set, mut id, mut workspace, mut thread) =
        (false, String::new(), String::new(), "keep-main".to_string());
    let mut json = false;
    let mut i = 0;
    while i < words.len() {
        let a = words[i];
        match a {
            "--workspace" => {
                let Some(v) = words.get(i + 1) else {
                    return Some(shift(a));
                };
                workspace = v.to_string();
                i += 2;
                continue;
            }
            "--json" => json = true,
            "--thread" => {
                let t = words.get(i + 1).copied().unwrap_or("");
                if !matches!(
                    t,
                    "keep-main" | "keep_main" | "adopt-sidecar" | "adopt_sidecar"
                ) {
                    return Some(usage(super::THREAD_USAGE.into()));
                }
                thread = t.to_string();
                i += 2;
                continue;
            }
            "set" if !set => set = true,
            "set" => id = "set".into(),
            _ if a.starts_with('-') => return Some(unknown_flag(a)),
            _ if set && id.is_empty() => id = a.to_string(),
            _ => return Some(usage(super::MAIN_USAGE.into())),
        }
        i += 1;
    }
    if !set {
        // A `set` that only `--workspace` or `--thread` read: not this one.
        return None;
    }
    if id.is_empty() {
        return Some(usage(main_set_usage()));
    }
    Some(Plan::Do(Write::MainSet {
        id,
        workspace,
        thread: thread.replace('-', "_"),
        json,
    }))
}

/// `mktemp -t …` has a folder to write in: `$TMPDIR`, else `/tmp`.
#[cfg(unix)]
pub fn temp_folder_usable() -> bool {
    use std::os::unix::ffi::OsStrExt;
    let dir = std::env::var_os("TMPDIR")
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "/tmp".into());
    let Ok(c) = std::ffi::CString::new(dir.as_bytes()) else {
        return false;
    };
    // SAFETY: access reads the NUL-ended path and nothing else.
    let writable = unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 };
    writable && Path::new(&dir).is_dir()
}

#[cfg(not(unix))]
pub fn temp_folder_usable() -> bool {
    true
}

/// Whether the plan reads `--brief -` where `mktemp` would fail.
pub fn needs_shell(w: &Write) -> bool {
    let brief = match w {
        Write::Open { brief, .. } | Write::New { brief, .. } => brief,
        Write::Live { path, .. } => return super::live::folder(path) == Some(None),
        _ => return false,
    };
    brief.file == "-" && !temp_folder_usable()
}

/// The brief's text for the body: `None` for none; `Err` with what the
/// shell prints (exit 2) when it can't be read or is not UTF-8.
fn brief_text(b: &Brief) -> Result<Option<String>, String> {
    let raw = if b.file == "-" {
        let mut raw = Vec::new();
        // `cat > "$tmp"`: a read error ends the copy there.
        let _ = std::io::stdin().lock().read_to_end(&mut raw);
        raw
    } else if !b.file.is_empty() {
        match std::fs::read(&b.file) {
            Ok(raw) => raw,
            Err(e) => return Err(Raise::at(10, pyjson::os_error(&e, &b.file).last_line()).text()),
        }
    } else if b.have_text {
        return Ok(Some(b.text.clone()));
    } else {
        return Ok(None);
    };
    String::from_utf8(raw)
        .map(Some)
        .map_err(|_| "brief_not_text: the brief is not UTF-8 text\n".into())
}

/// The request's body, as the shell's Python writes it. `Err`: what it
/// prints, then exit 2.
pub fn body(w: &Write, project: &str) -> Result<String, String> {
    let ws = |w: &str| s(if w.is_empty() { project } else { w });
    let v = match w {
        Write::Open {
            id,
            name,
            workspace,
            harness,
            model,
            force,
            brief,
            ..
        } => {
            let mut b = vec![
                ("workspace", ws(workspace)),
                ("id", s(id)),
                ("force", Value::Bool(*force)),
            ];
            for (k, v) in [("name", name), ("harness", harness), ("model", model)] {
                if !v.is_empty() {
                    b.push((k, s(v)));
                }
            }
            if let Some(t) = brief_text(brief)? {
                b.push(("brief", s(&t)));
            }
            confirm::object(b)
        }
        Write::New {
            name,
            harness,
            workspace,
            model,
            brief,
            ..
        } => {
            let mut b = vec![
                ("workspace", ws(workspace)),
                ("name", s(name)),
                ("harness", s(harness)),
                ("defaultHarness", Value::Bool(harness.is_empty())),
            ];
            if !model.is_empty() {
                b.push(("model", s(model)));
            }
            if let Some(t) = brief_text(brief)? {
                b.push(("brief", s(&t)));
            }
            confirm::object(b)
        }
        Write::Stop(n) => confirm::object(vec![
            ("workspace", ws(&n.workspace)),
            ("name", s(&n.name)),
            ("force", Value::Bool(n.switch)),
        ]),
        Write::Live { .. } => return Ok(String::new()),
        Write::MainSet {
            id,
            workspace,
            thread,
            ..
        } => confirm::object(vec![
            ("workspace", ws(workspace)),
            ("id", s(id)),
            ("thread", s(thread)),
        ]),
    };
    Ok(confirm::dumps(&v))
}

/// Steps 2 to 4 of the module's notes, on a connected client.
pub fn run(w: &Write, client: &mut Client, closing: &mut Closing, project: &str) -> Exit {
    if let Write::Live { path, mode } = w {
        let dir = super::live::folder(path).flatten();
        return super::live::run(client, closing, path, dir.as_deref(), *mode);
    }
    let body = match body(w, project) {
        Ok(b) => b,
        Err(text) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return Exit::Usage;
        }
    };
    let path = match w {
        Write::Open { .. } => OPEN,
        Write::New { .. } => NEW,
        Write::Stop(_) => STOP,
        Write::MainSet { .. } => MAIN_SET,
        Write::Live { .. } => return Exit::Error,
    };
    let split = match confirm::sidecar_call(client, true, path, &body) {
        Ok(s) => s,
        Err(exit) => return exit,
    };
    let json = match w {
        Write::Open { json, .. } | Write::New { json, .. } | Write::MainSet { json, .. } => *json,
        Write::Stop(n) => n.json,
        Write::Live { .. } => false,
    };
    let sessions = matches!(w, Write::Open { .. } | Write::MainSet { .. });
    if !split.ok() {
        let route_gone = split.status == "404"
            && (contains(&split.body, "route not found") || contains(&split.body, "Not Found"));
        if sessions && route_gone {
            out::stderr_line(TOO_OLD);
            return Exit::Error;
        }
        return fail(&split, client, closing);
    }
    if json {
        let mut text = split.body.clone();
        text.push(b'\n');
        return match out::write_flushed(&mut std::io::stdout().lock(), &text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    match w {
        Write::Open { .. } => lay_out(&split.body, open_lines).print(),
        Write::MainSet { .. } => lay_out(&split.body, main_set_lines).print(),
        Write::New { .. } => sidecar::lay_out(Reader::New, &split.body).print(),
        Write::Stop(_) => sidecar::lay_out(Reader::Stop, &split.body).print(),
        Write::Live { .. } => Exit::Error,
    }
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// `_sidecar_fail`: its line, the refusal's closing line, its exit.
fn fail(split: &Split, c: &Client, closing: &mut Closing) -> Exit {
    out::stderr_line(&fail_line(&split.status, &split.body));
    closing.answer(&split.response(), c.passport().is_some());
    exit_for(&split.status)
}

/// What the Python prints to each stream.
#[derive(Default)]
pub struct Lines {
    pub stdout: String,
    pub stderr: String,
}

/// `json.load(sys.stdin)` (line 4) and a reader, as the shell's Python.
fn lay_out(body: &[u8], f: fn(&Value, &mut Lines) -> Result<Exit, Raise>) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let v = match pyjson::loads(&raw) {
        Ok(v) => v,
        Err(e) => {
            return raised(
                String::new(),
                String::new(),
                &Raise::at(super::line::LOAD, e.last_line()),
            )
        }
    };
    let mut o = Lines::default();
    match f(&v, &mut o) {
        Ok(exit) => Shown {
            stdout: o.stdout,
            stderr: o.stderr,
            exit,
        },
        Err(r) => raised(o.stdout, o.stderr, &r),
    }
}

/// The lines of the shell's Python for these two verbs.
pub mod line {
    pub const O_ADDR: u32 = 103;
    pub const O_OPENED: u32 = 104;
    pub const O_WHAT: u32 = 105;
    pub const O_RESUMED: u32 = 106;
    pub const O_LIVE: u32 = 107;
    pub const O_ALREADY: u32 = 108;
    pub const O_ASLEEP: u32 = 110;
    pub const S_M: u32 = 129;
    pub const S_HEAD: u32 = 130;
    pub const S_WAS_IF: u32 = 131;
    pub const S_WAS: u32 = 132;
    pub const S_T: u32 = 133;
    pub const S_OTHER: u32 = 134;
    pub const S_KEEP: u32 = 135;
    pub const S_KEEP_ADDR: u32 = 137;
    pub const S_KEEP_LINE: u32 = 138;
    pub const S_ADOPT: u32 = 139;
    pub const S_ADOPT_ADDR: u32 = 141;
    pub const S_ADOPT_LINE: u32 = 142;
    pub const S_RESTART: u32 = 143;
}

/// `x.get(key)` at `line`: `None` (Python's) for a missing key.
fn get(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    Ok(py_get(v, key, line)?.cloned().unwrap_or(Value::Null))
}

/// `x[key]` at `line`, on a dict for sure.
fn index(v: &Value, key: &str, line: u32) -> Result<Value, Raise> {
    match py_get(v, key, line)? {
        Some(x) => Ok(x.clone()),
        None => Err(Raise::at(line, format!("KeyError: {}", py::repr(&s(key))))),
    }
}

fn str_of(v: &Value) -> String {
    py::str_of(v)
}

/// `x or {}`.
fn or_dict(v: Value) -> Value {
    if py::truthy(&v) {
        v
    } else {
        Value::Object(Vec::new())
    }
}

/// `_sessions_render open`.
pub fn open_lines(v: &Value, o: &mut Lines) -> Result<Exit, Raise> {
    use line::*;
    o.stdout
        .push_str(&format!("{}\n", str_of(&get(v, "address", O_ADDR)?)));
    if py::truthy(&get(v, "opened", O_OPENED)?) {
        let outcome = get(v, "outcome", O_WHAT)?;
        let what = if outcome == s("resumed_row") {
            "resumed its tab"
        } else {
            "reopened"
        };
        let addr = str_of(&index(v, "address", O_RESUMED)?);
        let cli = str_of(&get(v, "cli", O_RESUMED)?);
        o.stderr.push_str(&format!(
            "{what} {addr} ({cli}, same conversation). Message it: {CLI} msg {addr} \"\u{2026}\"\n"
        ));
    } else if py::truthy(&get(v, "alreadyLive", O_LIVE)?) {
        let addr = str_of(&index(v, "address", O_ALREADY)?);
        o.stderr.push_str(&format!(
            "already running as {addr}. Message it: {CLI} msg {addr} \"\u{2026}\"\n"
        ));
    } else {
        let addr = str_of(&index(v, "address", O_ASLEEP)?);
        let wake = get(v, "wakeWith", O_ASLEEP)?;
        let wake = if py::truthy(&wake) {
            str_of(&wake)
        } else {
            format!("{CLI} msg {addr} \"\u{2026}\"")
        };
        o.stderr.push_str(&format!(
            "{addr} is the main session (asleep). Wake it: {wake}\n"
        ));
    }
    Ok(Exit::Ok)
}

/// `_sessions_render main_set`.
pub fn main_set_lines(v: &Value, o: &mut Lines) -> Result<Exit, Raise> {
    use line::*;
    let m = or_dict(get(v, "main", S_M)?);
    let ws = get(v, "workspace", S_HEAD)?;
    let conv = get(&m, "conversationId", S_HEAD)?;
    let cli = get(&m, "cli", S_HEAD)?;
    let title = get(v, "title", S_HEAD)?;
    let title = super::cut(Some(&title), 60, S_HEAD)?;
    o.stdout.push_str(&format!(
        "{} main chat is now {} ({}) \"{}\"\n",
        str_of(&ws),
        str_of(&conv),
        str_of(&cli),
        title
    ));
    let prev = get(v, "previous", S_WAS_IF)?;
    if py::truthy(&prev)
        && !super::super::feedback::ask::py_eq(&prev, &get(&m, "conversationId", S_WAS_IF)?)
    {
        let p = index(v, "previous", S_WAS)?;
        let short = sidecar::py_slice(&p, 8, S_WAS)?;
        o.stdout.push_str(&format!(
            "Was: {} (its history stays; {CLI} sessions open {})\n",
            str_of(&p),
            str_of(&short)
        ));
    }
    let t = or_dict(get(v, "thread", S_T)?);
    let other = or_dict(get(&t, "other", S_OTHER)?);
    let choice = get(&t, "choice", S_KEEP)?;
    let lines = if choice == s("keep_main") {
        Some((
            "Thread: the main chat kept its Thread.",
            "The other chat\u{2019}s Thread is kept at",
            S_KEEP_ADDR,
            S_KEEP_LINE,
        ))
    } else if get(&t, "choice", S_ADOPT)? == s("adopt_sidecar") {
        Some((
            "Thread: the main chat now shows this chat\u{2019}s Thread.",
            "The old main Thread is kept at",
            S_ADOPT_ADDR,
            S_ADOPT_LINE,
        ))
    } else {
        None
    };
    if let Some((head, kept, addr_line, line)) = lines {
        o.stdout.push_str(head);
        o.stdout.push('\n');
        if py::truthy(&get(&other, "address", addr_line)?) {
            let a = str_of(&index(&other, "address", line)?);
            o.stdout
                .push_str(&format!("{kept} {a} (read it: {CLI} thread {a})\n"));
        }
    }
    let restart = get(v, "restartWith", S_RESTART)?;
    o.stdout.push_str(&format!(
        "It takes effect the next time the chat starts. Restart it now: {}\n",
        str_of(&restart)
    ));
    Ok(Exit::Ok)
}
