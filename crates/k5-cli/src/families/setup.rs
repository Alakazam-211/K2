//! `k5 setup`: the setup checklist on this server, and its writes, as the
//! shell CLI's `cmd_setup` does them (`cli/src/setup.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `_cli_apply_auth_for_verb setup <words>`: `setup` is an open
//!    tool, but with `chief-of-staff` as its first word, or as the first
//!    word after leading flags ([`names_locked_tool`]), it is the locked
//!    `agent-harness` tool, so in a session with `HOOK_SOCK` set and no
//!    passport it is refused ([`prelude::require_passport_lines`], exit
//!    3). Since 0.46.17 `_cli_check_session_passport` reads the same tool,
//!    so a passport whose socket is gone is probed first
//!    ([`prelude::id_tool`]), and a shared Codex app-server is the
//!    shell's.
//! 3. The words, as `cmd_setup`'s loop reads them ([`parse`]): `--json`;
//!    one verb of `hide`, `show`, `kessel`, `chief-of-staff`,
//!    `dismiss-offer` and `skip <step>`; `--tool`, `--name`, `--handle`,
//!    `--about` and `--focus-group`, each with a value, only after
//!    `kessel` or `chief-of-staff`. A mistake is its line (or the usage),
//!    exit 2, and nothing is sent.
//! 4. One `_setup_send` ([`crate::transport::Client::setup_send`]): over
//!    TCP, never the cell socket, the shell's TCP token as Bearer (in a
//!    session, `HOOK_SOCK` set, the passport, and with none no credential
//!    at all: `_setup_token`, 0.46.17) and `?format=text` but with
//!    `--json`. `setup` alone is `GET
//!    /cli/first-run/status`; each verb is a `POST` of its JSON body,
//!    built as `_setup_jstr` writes it ([`body`]). A POST is sent once:
//!    only a refused connect (nothing reached the daemon) is tried again,
//!    and a GET also after a 503 "migrating"; a refused passport is asked
//!    again once with a healed one (a 403 ran no handler).
//! 5. `_setup_render` ([`render`]) on what `raw=$(…)` holds: the answer,
//!    a newline and curl's status, after one `\n000` for each refused
//!    connect `_k2_curl` tried again ([`held`]). No answer (status 000, an
//!    empty body) is `Error: could not reach the K2 daemon (k2 setup)`,
//!    exit 1. A 200 prints the daemon's lines as they came (`--json`: its
//!    JSON and a newline); a 200 in JSON without `--json` is an older
//!    daemon's, read by the shell's Python ([`old_daemon`]). Any other
//!    status is the refusal's `code` and `hint` read with bash patterns,
//!    one `Error:` line, and exit 2 (usage), 4 (no such Focus Group), 3
//!    (refused) or 1.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]), from the body
//!    `_permission_note` was handed.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): a
//! command line with `-h`, `--help` or `help` anywhere in it (the front
//! sends those on, [`crate::front`]); a verb on the hand-off list
//! ([`SHELL_VERBS`]: `agent-reports`) where the loop reaches it; a word
//! that is not UTF-8; a host
//! setting that is not a plain host name or IPv4 address (bash hands curl
//! the URL as written); a `python3` the binary can't tell
//! ([`crate::families`], Without Python); and each form whose switch is
//! off ([`READ_NATIVE`], [`WRITE_NATIVE`]).
//!
//! Where this differs from the shell:
//!
//! - Without `python3` ([`crate::python::NATIVE_WITHOUT_PYTHON3`]) an
//!   older daemon's JSON list and a refusal's closing line print as they
//!   do with one; the shell there prints `python3: command not found`
//!   (exit 127) and no closing line.
//! - An answer cut short after its head (curl 18, or a reset mid-body):
//!   the shell prints the part that came with its status; this says the
//!   daemon could not be reached.
//! - A body with a NUL: bash 4.4 and later also print a warning of their
//!   own. A body that is not UTF-8, or a refusal with non-ASCII white
//!   space around its colons: bash's patterns there follow the C
//!   library's locale tables; this reads them as in the C locale but for
//!   the 200-character cut ([`cut_200`]), and the Python reads such
//!   bytes as U+FFFD. A refusal's words are lower-cased as ASCII (GNU
//!   `tr`; macOS's `tr` also lowers non-ASCII capitals). The daemon sends
//!   UTF-8 JSON with ASCII punctuation.
//! - `TMPDIR` unusable and an answer with a bulletin, a refused passport
//!   or a `permission` object: bash can't write its note file and prints
//!   its own error naming the script's process id.

use super::{curl_port, get, py_get, py_iter, py_len, raised, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::out::json::{self, Text, Value};
use crate::out::py;
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::text;
use crate::transport::{permission, Channel, Error, Response};
use k2_client::product_name;
use std::ffi::OsString;
use std::path::Path;

/// `k5 setup` and `k5 setup --json` (and every usage mistake, which sends
/// nothing) are the binary's. Off, the shell answers them.
pub const READ_NATIVE: bool = true;

/// The writes (`hide`, `show`, `skip`, `kessel`, `chief-of-staff`,
/// `dismiss-offer`) are the binary's. Off, the shell answers them.
pub const WRITE_NATIVE: bool = true;

/// The verbs of `cmd_setup` the shell answers (K2's ruling at 0.46.17):
/// where the loop reaches one with no verb before it, the command goes to
/// the shell, words after it and all (after another verb it is the
/// usage, exit 2, as for any second verb). `agent-reports` is
/// send-feedback's settings (`_sf_settings_run setup`). A test reads
/// `cmd_setup`'s arms: each word is parsed here or on this list.
pub const SHELL_VERBS: &[&str] = &["agent-reports"];

/// The tool `setup chief-of-staff` rides (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "agent-harness";

/// No answer.
pub const UNREACHABLE: &str = concat!(
    "Error: could not reach the ",
    product_name!(),
    " daemon (k2 setup)"
);
/// An answer that is not JSON, before its first 200 characters (bash) or
/// code points (the Python).
pub const NOT_UNDERSTOOD: &str = "Error: the daemon sent something k2 setup does not understand: ";
/// `skip` with no step.
pub const SKIP_USAGE: &str = "usage: k2 setup skip <step> (k2 setup lists them)";
/// `chief-of-staff` without its three flags.
pub const CHIEF_USAGE: &str = "usage: k2 setup chief-of-staff --name N --about \"what they do\" --tool claude|codex|grok [--focus-group G] (ask your person all three first)";
/// `--handle` after `chief-of-staff`.
pub const NO_HANDLE: &str = "k2 setup chief-of-staff: no --handle (its folder comes from --name)";
/// `--about` after `kessel`.
pub const NO_ABOUT: &str = "k2 setup kessel: no --about";

/// A Kessel flag after another verb.
pub fn goes_with(flag: &str) -> String {
    format!("k2 setup: {flag} goes with k2 setup kessel or k2 setup chief-of-staff")
}

/// A flag with nothing after it.
pub fn needs_a_value(flag: &str) -> String {
    format!("k2 setup: {flag} needs a value")
}

/// A word the loop doesn't take.
pub fn unknown_argument(word: &str) -> String {
    format!("k2 setup: unknown argument: {word}")
}

/// `_setup_usage`: `cli/text/help/setup.txt`.
pub fn usage() -> &'static str {
    text::help("setup").unwrap_or_default()
}

/// The verb, `cmd_setup`'s `$sub`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sub {
    /// No verb: the list.
    List,
    Hide,
    Show,
    Kessel,
    ChiefOfStaff,
    DismissOffer,
    Skip,
}

impl Sub {
    /// `$sub` as the shell holds it (the Python's `K2_SETUP_SUB`).
    pub fn word(self) -> &'static str {
        match self {
            Sub::List => "",
            Sub::Hide => "hide",
            Sub::Show => "show",
            Sub::Kessel => "kessel",
            Sub::ChiefOfStaff => "chief-of-staff",
            Sub::DismissOffer => "dismiss-offer",
            Sub::Skip => "skip",
        }
    }

    /// The route.
    pub fn endpoint(self) -> &'static str {
        match self {
            Sub::List => "/cli/first-run/status",
            Sub::Hide => "/cli/first-run/hide",
            Sub::Show => "/cli/first-run/show",
            Sub::Kessel => "/cli/first-run/kessel",
            Sub::ChiefOfStaff => "/cli/first-run/chief-of-staff",
            Sub::DismissOffer => "/cli/first-run/offer/dismiss",
            Sub::Skip => "/cli/first-run/skip",
        }
    }

    /// A write: a POST.
    pub fn post(self) -> bool {
        self != Sub::List
    }

    /// The Python's `made`: an answer about an agent it made.
    pub fn made(self) -> bool {
        matches!(self, Sub::Kessel | Sub::ChiefOfStaff)
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub sub: Sub,
    pub json: bool,
    /// The POST body; empty for the list.
    pub body: String,
}

/// Why the words send nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// `-h`, `--help` or `help` where the loop reads a word: the shell's.
    Help,
    /// A verb of [`SHELL_VERBS`] where the loop reads a word: the shell's.
    Shell,
    /// What goes on stderr; exit 2.
    Usage(String),
}

/// The words after `setup`, read as `cmd_setup`'s loop reads them.
pub fn parse(words: &[&str]) -> Result<Line, Stop> {
    let usage_err = || Stop::Usage(usage().to_string());
    let line_err = |l: String| Stop::Usage(format!("{l}\n"));
    let mut sub: Option<Sub> = None;
    let mut json = false;
    let (mut tool, mut name, mut handle, mut about, mut group) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        String::new(),
    );
    let mut have_about = false;
    let mut row = String::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "-h" | "--help" | "help" => return Err(Stop::Help),
            "--json" => {
                json = true;
                i += 1;
            }
            "hide" | "show" | "kessel" | "chief-of-staff" | "dismiss-offer" => {
                if sub.is_some() {
                    return Err(usage_err());
                }
                sub = Some(match w {
                    "hide" => Sub::Hide,
                    "show" => Sub::Show,
                    "kessel" => Sub::Kessel,
                    "chief-of-staff" => Sub::ChiefOfStaff,
                    _ => Sub::DismissOffer,
                });
                i += 1;
            }
            "--tool" | "--name" | "--handle" | "--about" | "--focus-group" => {
                if !matches!(sub, Some(Sub::Kessel | Sub::ChiefOfStaff)) {
                    return Err(line_err(goes_with(w)));
                }
                let Some(v) = words.get(i + 1) else {
                    return Err(line_err(needs_a_value(w)));
                };
                match w {
                    "--tool" => tool = (*v).to_string(),
                    "--name" => name = (*v).to_string(),
                    "--focus-group" => group = (*v).to_string(),
                    "--handle" => {
                        if sub != Some(Sub::Kessel) {
                            return Err(line_err(NO_HANDLE.to_string()));
                        }
                        handle = (*v).to_string();
                    }
                    _ => {
                        if sub != Some(Sub::ChiefOfStaff) {
                            return Err(line_err(NO_ABOUT.to_string()));
                        }
                        about = (*v).to_string();
                        have_about = true;
                    }
                }
                i += 2;
            }
            w if SHELL_VERBS.contains(&w) => {
                if sub.is_some() {
                    return Err(usage_err());
                }
                return Err(Stop::Shell);
            }
            "skip" => {
                if sub.is_some() {
                    return Err(usage_err());
                }
                sub = Some(Sub::Skip);
                i += 1;
                match words.get(i) {
                    Some(r) if !r.is_empty() => row = (*r).to_string(),
                    _ => return Err(line_err(SKIP_USAGE.to_string())),
                }
                i += 1;
            }
            _ => return Err(Stop::Usage(format!("{}\n{}", unknown_argument(w), usage()))),
        }
    }
    let sub = sub.unwrap_or(Sub::List);
    if sub == Sub::ChiefOfStaff && (name.is_empty() || !have_about || tool.is_empty()) {
        return Err(line_err(CHIEF_USAGE.to_string()));
    }
    let body = match sub {
        Sub::Kessel | Sub::ChiefOfStaff => {
            let mut fields = Vec::new();
            if !tool.is_empty() {
                fields.push(format!("\"tool\": {}", jstr(&tool)));
            }
            if !name.is_empty() {
                fields.push(format!("\"name\": {}", jstr(&name)));
            }
            if !handle.is_empty() {
                fields.push(format!("\"handle\": {}", jstr(&handle)));
            }
            if !about.is_empty() || sub == Sub::ChiefOfStaff {
                fields.push(format!("\"about\": {}", jstr(&about)));
            }
            if !group.is_empty() {
                fields.push(format!("\"focusGroup\": {}", jstr(&group)));
            }
            format!("{{{}}}", fields.join(", "))
        }
        Sub::Skip => format!("{{\"row\": {}}}", jstr(&row)),
        Sub::Hide | Sub::Show | Sub::DismissOffer => "{}".to_string(),
        Sub::List => String::new(),
    };
    Ok(Line { sub, json, body })
}

/// `_setup_jstr`: `value` as a JSON string, quotes included. In the C
/// locale: `\` and `"` escaped, newline, tab and carriage return as
/// `\n`, `\t`, `\r`, DEL as it is, every other control byte `\u00xx`
/// (lower-case hex), all else (UTF-8 too) as it is.
pub fn jstr(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{7f}' => out.push(c),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A host bash hands curl in the URL the same way this connects to it: a
/// name or IPv4 address of letters, digits, `.` and `-`.
pub fn plain_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// `k5 setup ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help" | "help")) {
        return None;
    }
    let read = parse(&words);
    if matches!(read, Err(Stop::Shell)) {
        return None;
    }
    let native = match &read {
        Ok(line) if line.sub.post() => WRITE_NATIVE,
        _ => READ_NATIVE,
    };
    if !native || !python::family_native(python::here()) {
        return None;
    }
    let hook_sock = super::hook_sock();
    // The locked tool under a shared Codex app-server: the shell's check.
    if names_locked_tool(&words)
        && hook_sock.is_some()
        && prelude::codex_env(std::env::vars_os().map(|(name, _)| name))
    {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    if prelude::gate(&settings).is_none() && !plain_host(&settings.host) {
        return None;
    }
    Some(run_with(
        args,
        &words,
        read,
        hook_sock.as_deref().map(Path::new),
        settings,
    ))
}

/// `_cli_tool_id_for_args setup <words>` names the locked tool (0.46.17):
/// the first word is `chief-of-staff`, or it is the first word after the
/// leading flags (each of `--tool`, `--name`, `--handle`, `--about` and
/// `--focus-group` takes the word after it; `--` ends the flags; `-h` or
/// `--help` stops the search and names none).
pub fn names_locked_tool(words: &[&str]) -> bool {
    if words.first() == Some(&"chief-of-staff") {
        return true;
    }
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "-h" | "--help" => return false,
            "--" => {
                i += 1;
                break;
            }
            "--tool" | "--name" | "--handle" | "--about" | "--focus-group" => {
                i += if i + 1 < words.len() { 2 } else { 1 };
            }
            w if w.starts_with('-') => i += 1,
            _ => break,
        }
    }
    words.get(i) == Some(&"chief-of-staff")
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    read: Result<Line, Stop>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    // The locked tool, read as `_cli_tool_id_for_args` reads it: the
    // stale-passport probe, then the require-passport refusal (0.46.17,
    // `_cli_check_session_passport` reads the same tool).
    if names_locked_tool(words) {
        if let Some(stop) = prelude::id_tool(
            &settings,
            hook_sock,
            passport::current(args).would_refuse(),
            TOOL,
        ) {
            return stop;
        }
    }
    let line = match read {
        Ok(line) => line,
        Err(Stop::Usage(text)) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return Exit::Usage;
        }
        // `run` hands these to the shell before this.
        Err(Stop::Help | Stop::Shell) => return Exit::Usage,
    };
    // A port curl won't take: curl sends nothing, and `_setup_render`
    // reads no status.
    let Some(port) = settings.port_text.as_deref().and_then(curl_port) else {
        out::stderr_line(UNREACHABLE);
        return Exit::Error;
    };
    settings.port = Some(port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let result = client.setup_send(
        line.sub.post(),
        line.sub.endpoint(),
        !line.json,
        line.body.as_bytes(),
    );
    let raw = held(&result, client.refused_before_answer());
    let rendered = render(line.sub, line.json, &raw, utf8_locale);
    let mut closing = Closing::default();
    if let Some(body) = &rendered.noted {
        let answer = Response {
            status: result.as_ref().map_or(0, |r| r.status),
            body: body.clone(),
            channel: Channel::Tcp,
            permission: permission::note(body),
            server_version: None,
        };
        closing.answer(&answer, client.passport().is_some());
    }
    let exit = rendered.print();
    closing.flush(&mut client, exit);
    exit
}

/// What `raw=$(_setup_send …)` holds: one `\n000` for each refused
/// connect `_k2_curl` tried again, the answer and `\n<status>` (curl's
/// `-w`); `\n000` alone when curl failed. NULs and trailing newlines are
/// dropped, as `$(…)` drops them.
pub fn held(result: &Result<Response, Error>, refused: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    match result {
        Ok(r) => {
            for _ in 0..refused {
                raw.extend_from_slice(b"\n000");
            }
            raw.extend_from_slice(&r.body);
            raw.extend_from_slice(format!("\n{:03}", r.status).as_bytes());
        }
        Err(_) => raw.extend_from_slice(b"\n000"),
    }
    raw.retain(|b| *b != 0);
    while raw.last() == Some(&b'\n') {
        raw.pop();
    }
    raw
}

/// What the command prints, and the body handed to `_permission_note`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// `None` when nothing was noted (no answer).
    pub noted: Option<Vec<u8>>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: Exit,
}

impl Rendered {
    fn err(noted: Option<Vec<u8>>, stderr: Vec<u8>, exit: Exit) -> Rendered {
        Rendered {
            noted,
            stdout: Vec::new(),
            stderr,
            exit,
        }
    }

    /// Writes stdout, then stderr; the exit. A broken stdout ends as
    /// [`out::after_stdout_error`] says.
    pub fn print(&self) -> Exit {
        if !self.stdout.is_empty() {
            if let Err(e) = out::write_flushed(&mut std::io::stdout().lock(), &self.stdout) {
                return out::after_stdout_error(&e);
            }
        }
        if !self.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), &self.stderr);
        }
        self.exit
    }
}

/// `[[:space:]]` in the C locale.
fn c_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|at| at + from)
}

fn skip_space(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && c_space(b[i]) {
        i += 1;
    }
    i
}

/// Where `"<key>"[[:space:]]*:[[:space:]]*` ends, for each place `"<key>"`
/// is in `text`, first to last, when a `:` follows it so.
fn after_key<'a>(text: &'a [u8], key: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    let quoted = [b"\"", key, b"\""].concat();
    let mut from = 0;
    std::iter::from_fn(move || loop {
        let at = find(text, &quoted, from)?;
        from = at + 1;
        let i = skip_space(text, at + quoted.len());
        if text.get(i) == Some(&b':') {
            return Some(skip_space(text, i + 1));
        }
    })
}

/// `[[ $text =~ '"error"[[:space:]]*:[[:space:]]*\{(.*)$' ]]`: what the
/// group holds, everything after the first such `{`.
pub fn error_object(text: &[u8]) -> Option<&[u8]> {
    after_key(text, b"error")
        .find(|i| text.get(*i) == Some(&b'{'))
        .map(|i| &text[i + 1..])
}

/// `[[ $text =~ '"error"[[:space:]]*:[[:space:]]*"' ]]`.
pub fn error_string(text: &[u8]) -> bool {
    after_key(text, b"error").any(|i| text.get(i) == Some(&b'"'))
}

/// `_setup_jfield KEY TEXT`: the first `"KEY"[[:space:]]*:[[:space:]]*"…"`
/// in `text` (its body any run of bytes but `"` and `\`, or `\` and one
/// more), decoded by [`junesc`]. `None` when there is none.
pub fn jfield(key: &str, text: &[u8]) -> Option<Vec<u8>> {
    after_key(text, key.as_bytes()).find_map(|i| {
        if text.get(i) != Some(&b'"') {
            return None;
        }
        let start = i + 1;
        let mut j = start;
        while j < text.len() {
            match text[j] {
                b'"' => return Some(junesc(&text[start..j])),
                b'\\' if j + 1 < text.len() => j += 2,
                b'\\' => return None,
                _ => j += 1,
            }
        }
        None
    })
}

/// `_setup_junesc`, byte by byte (it runs in the C locale): `\n`, `\t`,
/// `\r`, `\b`, `\f`; `\uXXXX` (four hex digits) as UTF-8, a surrogate half
/// as its own three bytes, U+0000 as nothing; `\u` and the four bytes
/// after it dropped when they are not four hex digits; `\` and any other
/// byte, that byte.
pub fn junesc(s: &[u8]) -> Vec<u8> {
    if !s.contains(&b'\\') {
        return s.to_vec();
    }
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        if c != b'\\' {
            out.push(c);
            i += 1;
            continue;
        }
        i += 1;
        let Some(&c) = s.get(i) else {
            break;
        };
        match c {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'b' => out.push(0x08),
            b'f' => out.push(0x0c),
            b'u' => {
                let hex = s.get(i + 1..(i + 5).min(s.len())).unwrap_or_default();
                i += 4;
                if hex.len() == 4 && hex.iter().all(u8::is_ascii_hexdigit) {
                    let cp = u32::from_str_radix(std::str::from_utf8(hex).unwrap_or("0"), 16)
                        .unwrap_or(0);
                    out.extend(utf8_bytes(cp));
                }
            }
            other => out.push(other),
        }
        i += 1;
    }
    out
}

/// `_setup_junesc`'s bytes for a code point below U+10000: one, two or
/// three bytes as UTF-8 builds them (a surrogate half too), none for 0.
fn utf8_bytes(cp: u32) -> Vec<u8> {
    // Each byte fits: cp < 0x10000.
    let b = |v: u32| (v & 0xff) as u8;
    if cp == 0 {
        Vec::new()
    } else if cp < 0x80 {
        vec![b(cp)]
    } else if cp < 0x800 {
        vec![b(0xc0 | (cp >> 6)), b(0x80 | (cp & 0x3f))]
    } else {
        vec![
            b(0xe0 | (cp >> 12)),
            b(0x80 | ((cp >> 6) & 0x3f)),
            b(0x80 | (cp & 0x3f)),
        ]
    }
}

/// `${body:0:200}`: 200 characters in a UTF-8 locale, where a byte that
/// starts no whole character counts as one (bash's `ADVANCE_CHAR`), else
/// 200 bytes. `utf8` is asked only when the two could differ.
pub fn cut_200(body: &[u8], utf8: impl FnOnce() -> bool) -> &[u8] {
    const N: usize = 200;
    if body.len() <= N || body[..N].is_ascii() || !utf8() {
        return &body[..body.len().min(N)];
    }
    let mut i = 0;
    for _ in 0..N {
        if i >= body.len() {
            break;
        }
        i += char_len(&body[i..]);
    }
    &body[..i]
}

/// The bytes of the character `b` starts with, or 1 when it starts none.
fn char_len(b: &[u8]) -> usize {
    let want = match b[0] {
        0x00..=0x7f => return 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return 1,
    };
    match b.get(..want) {
        Some(c) if std::str::from_utf8(c).is_ok() => want,
        _ => 1,
    }
}

/// Whether the C library's character type for this environment is UTF-8,
/// as bash's own `setlocale(LC_ALL, "")` finds it.
#[cfg(unix)]
pub fn utf8_locale() -> bool {
    use std::ffi::CStr;
    // SAFETY: setlocale and nl_langinfo get NUL-terminated strings and
    // return pointers into the C library's own buffers, copied at once;
    // the old LC_CTYPE is put back before returning. Nothing else in this
    // process reads the locale meanwhile.
    unsafe {
        let old = libc::setlocale(libc::LC_CTYPE, std::ptr::null());
        let saved = (!old.is_null()).then(|| CStr::from_ptr(old).to_owned());
        let set = libc::setlocale(libc::LC_CTYPE, c"".as_ptr());
        let utf8 = !set.is_null() && {
            let cs = libc::nl_langinfo(libc::CODESET);
            !cs.is_null() && {
                let name = CStr::from_ptr(cs).to_bytes().to_ascii_lowercase();
                name == b"utf-8" || name == b"utf8"
            }
        };
        if let Some(s) = saved {
            libc::setlocale(libc::LC_CTYPE, s.as_ptr());
        }
        utf8
    }
}

/// Windows: the family never runs natively there ([`crate::python`]).
#[cfg(not(unix))]
pub fn utf8_locale() -> bool {
    false
}

/// `_setup_render <sub> <as_json> <raw>` on `raw` as [`held`] gives it.
/// `utf8`: [`utf8_locale`], asked only for a cut that depends on it.
pub fn render(sub: Sub, json: bool, raw: &[u8], utf8: fn() -> bool) -> Rendered {
    let (body, code) = match raw.iter().rposition(|b| *b == b'\n') {
        Some(i) => (&raw[..i], &raw[i + 1..]),
        None => (raw, raw),
    };
    if raw.is_empty() || code == raw || code == b"000" || body.is_empty() {
        return Rendered::err(None, format!("{UNREACHABLE}\n").into_bytes(), Exit::Error);
    }
    let noted = Some(body.to_vec());
    let lead = body.iter().take_while(|b| c_space(**b)).count();
    let object = body.get(lead) == Some(&b'{');
    if code == b"200" {
        if json {
            let mut stdout = body.to_vec();
            stdout.push(b'\n');
            return Rendered {
                noted,
                stdout,
                stderr: Vec::new(),
                exit: Exit::Ok,
            };
        }
        if object {
            let shown = old_daemon(sub, body);
            return Rendered {
                noted,
                stdout: shown.stdout.into_bytes(),
                stderr: shown.stderr.into_bytes(),
                exit: shown.exit,
            };
        }
        return Rendered {
            noted,
            stdout: body.to_vec(),
            stderr: Vec::new(),
            exit: Exit::Ok,
        };
    }
    if !object {
        let mut stderr = NOT_UNDERSTOOD.as_bytes().to_vec();
        stderr.extend_from_slice(cut_200(body, utf8));
        stderr.push(b'\n');
        return Rendered::err(noted, stderr, Exit::Error);
    }
    let mut err_code = Vec::new();
    let hint = if let Some(rest) = error_object(body) {
        if let Some(c) = jfield("code", rest) {
            err_code = c;
        }
        match jfield("hint", rest).filter(|h| !h.is_empty()) {
            Some(h) => h,
            None => jfield("message", rest)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| err_code.clone()),
        }
    } else if let Some(e) = error_string(body).then(|| jfield("error", body)).flatten() {
        e
    } else {
        cut_200(body, utf8).to_vec()
    };
    let mut stderr = b"Error: ".to_vec();
    if !err_code.is_empty() && find(&hint, &err_code, 0).is_none() {
        stderr.extend_from_slice(&err_code);
        stderr.extend_from_slice(b": ");
    }
    stderr.extend_from_slice(&hint);
    stderr.push(b'\n');
    Rendered::err(noted, stderr, refusal_exit(&err_code, &hint))
}

/// `_setup_render`'s exit for a refusal's code and hint.
pub fn refusal_exit(code: &[u8], hint: &[u8]) -> Exit {
    match code {
        b"usage" | b"unknown_row" | b"tool_not_signed_in" => return Exit::Usage,
        b"unknown_focus_group" | b"ambiguous_focus_group" => return Exit::NotFound,
        _ => {}
    }
    let mut low = code.to_ascii_lowercase();
    low.push(b' ');
    low.extend(hint.to_ascii_lowercase());
    let refused = REFUSED_WORDS
        .iter()
        .any(|w| find(&low, w.as_bytes(), 0).is_some());
    if refused {
        Exit::Refused
    } else {
        Exit::Error
    }
}

/// A refusal holding one of these (lower-cased) is exit 3, in bash and in
/// the Python alike.
pub const REFUSED_WORDS: [&str; 6] = [
    "person_only",
    "session_only",
    "forbidden",
    "role_required",
    "token",
    "passport",
];

/// The Python's line numbers where it can raise (`tests` pins them).
pub const PY_ERROR_LINE: u32 = 18;
pub const PY_LOW: u32 = 19;
pub const PY_GRANTED: u32 = 34;
pub const PY_LEFT: u32 = 35;
pub const PY_JOIN_GRANTED: u32 = 37;
pub const PY_JOIN_LEFT: u32 = 39;
pub const PY_DONE: u32 = 61;
pub const PY_LINE: u32 = 65;
pub const PY_NOTE: u32 = 66;
pub const PY_KESSEL: u32 = 83;

/// `_setup_render_json_py <sub> 0` on `body`: the list from a daemon
/// before `?format=text`, as the shell's Python prints it. A body that is
/// not UTF-8 is read with U+FFFD for each bad byte.
pub fn old_daemon(sub: Sub, body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let raw200: String = raw.chars().take(200).collect();
    let Ok(d) = json::parse(&raw) else {
        return Shown::err(format!("{NOT_UNDERSTOOD}{raw200}\n"), Exit::Error);
    };
    let mut p = Py::default();
    match p.lines(sub, &d, &raw200) {
        Ok(exit) => Shown {
            stdout: p.out,
            stderr: p.err,
            exit,
        },
        Err(r) => raised(p.out, p.err, &r),
    }
}

/// The Python's two streams so far.
#[derive(Default)]
struct Py {
    out: String,
    err: String,
}

fn none() -> Value {
    Value::Null
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `d.get(key)` on a value already known to be a dict.
fn dget<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `d.get(key) or default`.
fn or<'a>(v: Option<&'a Value>, default: &'a Value) -> &'a Value {
    v.filter(|v| py::truthy(v)).unwrap_or(default)
}

/// `a == b` as Python compares two `json.loads` values.
fn py_eq(a: &Value, b: &Value) -> bool {
    let num = |v: &Value| -> Option<f64> {
        match v {
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Int(d) => d.parse().ok(),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    };
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| get(y, &text_key(k)).is_some_and(|w| py_eq(v, w)))
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

/// A key as [`get`] looks it up.
fn text_key(t: &Text) -> String {
    t.as_str().map_or_else(
        || {
            t.code_points()
                .into_iter()
                .map(|cp| char::from_u32(cp).unwrap_or('\u{fffd}'))
                .collect()
        },
        str::to_string,
    )
}

/// `v` is a string equal to `text`.
fn is_str(v: Option<&Value>, text: &str) -> bool {
    matches!(v, Some(Value::Str(t)) if t.as_str() == Some(text))
}

/// `hash(v)` raises for a list or a dict.
fn hashable(v: &Value, line: u32) -> Result<(), Raise> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            line,
            format!("TypeError: unhashable type: '{}'", py::type_name(v)),
        )),
        _ => Ok(()),
    }
}

/// `x in y`.
fn py_in(x: &Value, y: &Value, line: u32) -> Result<bool, Raise> {
    match y {
        Value::Str(hay) => {
            let Value::Str(needle) = x else {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: 'in <string>' requires string as left operand, not {}",
                        py::type_name(x)
                    ),
                ));
            };
            let (h, n) = (hay.code_points(), needle.code_points());
            Ok(n.is_empty() || h.windows(n.len()).any(|w| w == n.as_slice()))
        }
        Value::Array(items) => Ok(items.iter().any(|e| py_eq(x, e))),
        Value::Object(pairs) => {
            hashable(x, line)?;
            Ok(match x {
                Value::Str(k) => pairs.iter().any(|(key, _)| key == k),
                _ => false,
            })
        }
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: argument of type '{}' is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `x + "<s>"`.
fn add_str(x: &Value, s: &str, line: u32) -> Result<String, Raise> {
    match x {
        Value::Str(_) => Ok(format!("{}{s}", py::str_of(x))),
        Value::Array(_) => Err(Raise::at(
            line,
            "TypeError: can only concatenate list (not \"str\") to list",
        )),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: unsupported operand type(s) for +: '{}' and 'str'",
                py::type_name(other)
            ),
        )),
    }
}

/// `"<s>" + y`, `s` a str.
fn str_add(s: String, y: &Value, line: u32) -> Result<String, Raise> {
    match y {
        Value::Str(_) => Ok(s + &py::str_of(y)),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `", ".join(v)`.
fn join(v: &Value, line: u32) -> Result<String, Raise> {
    if !matches!(v, Value::Str(_) | Value::Array(_) | Value::Object(_)) {
        return Err(Raise::at(line, "TypeError: can only join an iterable"));
    }
    let items = py_iter(v, line)?;
    let mut parts = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        if !matches!(item, Value::Str(_)) {
            return Err(Raise::at(
                line,
                format!(
                    "TypeError: sequence item {i}: expected str instance, {} found",
                    py::type_name(item)
                ),
            ));
        }
        parts.push(py::str_of(item));
    }
    Ok(parts.join(", "))
}

/// An exception raised inside a generator expression on `line`.
fn in_genexpr(line: u32, r: Raise) -> Raise {
    Raise::inside(line, line, "<genexpr>", r.exception)
}

/// The step labels the Python knows.
const LABELS: [(&str, &str); 7] = [
    ("dev_tools", "Install developer tools"),
    ("llm_sign_in", "Sign in to an AI tool"),
    ("first_agent", "Make your first agent"),
    ("chief_of_staff", "Hire your Chief of Staff"),
    ("tunnel", "Link your subdomain"),
    ("phone", "Connect your phone"),
    ("own_sign_in", "Make your own sign-in"),
];

/// The notes the Python knows.
const NOTES: [(&str, &str); 3] = [
    (
        "needs_tunnel",
        "Away from home, your phone needs your subdomain first.",
    ),
    ("needs_kessel", "You will meet Kessel first."),
    (
        "installing",
        "The Apple installer is open. Follow it, and this step finishes by itself.",
    ),
];

/// A row's mark by its state.
const MARKS: [(&str, &str); 3] = [("done", "[x]"), ("todo", "[ ]"), ("skipped", "[-]")];

/// The Apple tools' line.
pub const APPLE_TOOLS: &str =
    "The Apple tools include git, which workspaces need: k2 git-tools install";
pub const FINISHED: &str =
    "Finished: the app no longer shows this list (k2 setup show brings it back).";
pub const HIDDEN: &str = "Hidden in the app (k2 setup show brings it back).";
pub const NOT_ENABLED: &str = "The app does not show setup lists on this server yet.";
pub const MEET_KESSEL: &str = concat!(
    "Next: meet Kessel, the agent that runs ",
    product_name!(),
    ": k2 setup kessel"
);

/// `TABLE.get(v)` for a value the Python hashes first.
fn table_get(
    table: &'static [(&'static str, &'static str)],
    v: &Value,
    line: u32,
) -> Result<Option<&'static str>, Raise> {
    hashable(v, line)?;
    Ok(match v {
        Value::Str(t) => t
            .as_str()
            .and_then(|k| table.iter().find(|(key, _)| *key == k))
            .map(|(_, label)| *label),
        _ => None,
    })
}

impl Py {
    fn print(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }

    /// The Python after `json.loads`: its exit, or where it raised.
    fn lines(&mut self, sub: Sub, d: &Value, raw200: &str) -> Result<Exit, Raise> {
        let dict = matches!(d, Value::Object(_));
        let err = dget(d, "error").filter(|v| !matches!(v, Value::Null));
        let made = sub.made();
        let has = |k: &str| dget(d, k).is_some();
        if err.is_some() || !dict || (!has("rows") && !(made && has("workspaceId"))) {
            return self.error(err, raw200);
        }
        if made {
            return self.made(sub, d);
        }
        self.rows(d)
    }

    fn error(&mut self, err: Option<&Value>, raw200: &str) -> Result<Exit, Raise> {
        let empty = s("");
        let (code, hint) = match err {
            Some(e) if matches!(e, Value::Object(_)) => {
                let code = or(dget(e, "code"), &empty).clone();
                let hint = or(dget(e, "hint"), or(dget(e, "message"), &code)).clone();
                (code, hint)
            }
            Some(other) => (empty.clone(), s(&py::str_of(other))),
            None => (empty.clone(), s(raw200)),
        };
        let prefix = if py::truthy(&code) && !py_in(&code, &hint, PY_ERROR_LINE)? {
            add_str(&code, ": ", PY_ERROR_LINE)?
        } else {
            String::new()
        };
        self.err
            .push_str(&format!("Error: {prefix}{}\n", py::str_of(&hint)));
        let low = str_add(add_str(&code, " ", PY_LOW)?, &hint, PY_LOW)?.to_lowercase();
        let code = match &code {
            Value::Str(t) => t.as_str().unwrap_or_default(),
            _ => "",
        };
        Ok(match code {
            "usage" | "unknown_row" | "tool_not_signed_in" => Exit::Usage,
            "unknown_focus_group" | "ambiguous_focus_group" => Exit::NotFound,
            _ if REFUSED_WORDS.iter().any(|w| low.contains(w)) => Exit::Refused,
            _ => Exit::Error,
        })
    }

    fn made(&mut self, sub: Sub, d: &Value) -> Result<Exit, Raise> {
        let (nothing, empty_dict, empty_list) =
            (none(), Value::Object(Vec::new()), Value::Array(Vec::new()));
        let perms = or(dget(d, "permissions"), &empty_dict);
        let what = if sub == Sub::Kessel {
            "Kessel"
        } else {
            "your Chief of Staff"
        };
        let field = |k: &str| py::str_of(dget(d, k).unwrap_or(&nothing));
        self.print(&format!(
            "Made {what}: {} ({}) at {}",
            field("name"),
            field("handle"),
            field("path")
        ));
        let granted = or(py_get(perms, "granted", PY_GRANTED)?, &empty_list);
        let left = or(py_get(perms, "left", PY_LEFT)?, &empty_list);
        if py::truthy(granted) {
            let joined = join(granted, PY_JOIN_GRANTED)?;
            self.print(&format!("Allowed from the start: {joined}"));
        }
        if py::truthy(left) {
            let mut parts = Vec::new();
            for l in py_iter(left, PY_JOIN_LEFT)? {
                let action = py_get(&l, "action", PY_JOIN_LEFT)
                    .map_err(|r| in_genexpr(PY_JOIN_LEFT, r))?
                    .unwrap_or(&nothing);
                let reason = py_get(&l, "reason", PY_JOIN_LEFT)
                    .map_err(|r| in_genexpr(PY_JOIN_LEFT, r))?
                    .unwrap_or(&nothing);
                parts.push(format!("{} ({})", py::str_of(action), py::str_of(reason)));
            }
            self.print(&format!(
                "It asks on a permission card for: {}",
                parts.join(", ")
            ));
        }
        if sub == Sub::Kessel {
            self.print(&format!(
                "Kessel is starting and will say hello on its Thread: k2 thread {}",
                field("handle")
            ));
        } else {
            self.print(&format!(
                "Tell your person on your Thread, and point them to {} (k2 msg {}).",
                field("name"),
                field("handle")
            ));
        }
        Ok(Exit::Ok)
    }

    fn rows(&mut self, d: &Value) -> Result<Exit, Raise> {
        let nothing = none();
        let empty_list = Value::Array(Vec::new());
        let rows = or(dget(d, "rows"), &empty_list);
        let items = py_iter(rows, PY_DONE)?;
        let mut done = 0;
        for r in &items {
            let state = py_get(r, "state", PY_DONE).map_err(|e| in_genexpr(PY_DONE, e))?;
            if is_str(state, "done") {
                done += 1;
            }
        }
        let total = py_len(rows, PY_DONE + 1)?;
        self.print(&format!("Setup: {done} of {total} done"));
        let question = s("?");
        for r in &items {
            let rid = or(dget(r, "id"), &question);
            let state = dget(r, "state").unwrap_or(&nothing);
            let mark = table_get(&MARKS, state, PY_LINE)?.unwrap_or("[?]");
            let label = match table_get(&LABELS, rid, PY_LINE)? {
                Some(l) => l.to_string(),
                None => py::str_of(rid),
            };
            let mut line = format!("  {mark} {label}");
            let empty = s("");
            let note_key = or(dget(r, "note"), &empty);
            let mut note = table_get(&NOTES, note_key, PY_NOTE)?.map(str::to_string);
            let dev = is_str(Some(rid), "dev_tools");
            let opens = dget(r, "opens");
            let raw_note = dget(r, "note");
            if dev
                && opens.is_none_or(|o| matches!(o, Value::Null))
                && raw_note.is_some_and(py::truthy)
            {
                note = Some(format!("run: {}", py::str_of(raw_note.unwrap_or(&nothing))));
            } else if dev && is_str(opens, "dev_tools") && note.is_none() {
                note = Some(APPLE_TOOLS.to_string());
            }
            if let Some(n) = note.filter(|n| !n.is_empty()) {
                if !is_str(dget(r, "state"), "done") {
                    line = format!("{line} ({n})");
                }
            }
            self.print(&line);
        }
        let truthy = |k: &str| dget(d, k).is_some_and(py::truthy);
        if truthy("finished") {
            self.print(FINISHED);
        } else if truthy("hidden") {
            self.print(HIDDEN);
        }
        if !truthy("enabled") {
            self.print(NOT_ENABLED);
        }
        if let Some(kessel) = dget(d, "kessel").filter(|k| py::truthy(k)) {
            let name = py_get(kessel, "name", PY_KESSEL)?.unwrap_or(&nothing);
            let path = py_get(kessel, "path", PY_KESSEL)?.unwrap_or(&nothing);
            self.print(&format!(
                "Kessel: {} at {} (k2 thread <its handle> to talk to it)",
                py::str_of(name),
                py::str_of(path)
            ));
        }
        if is_str(dget(d, "offer"), "meet_kessel") {
            self.print(MEET_KESSEL);
        }
        Ok(Exit::Ok)
    }
}

#[cfg(test)]
mod tests;
