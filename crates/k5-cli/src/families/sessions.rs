//! `k5 sessions` and its read forms, as the shell CLI's `cmd_sessions`
//! answers them (`cli/src/awareness.sh`, `cli/src/sessions.sh`): the
//! roster (no verb, a first word that is a flag, `list`, `ls`), past
//! conversations (`past`, or any of `--past`, `--search`, `--days`,
//! `--cli`, `--limit`, `--archived`), `show <id>`, and `main` without
//! `set`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    the 0.40.4 rename line, and the two gates ([`crate::prelude`]).
//! 2. `sessions` is an `id` tool (`sessions-spawn` in the shell's policy),
//!    so with `HOOK_SOCK` set: the stale-passport or require-passport
//!    lines, exit 3 ([`crate::prelude::id_tool`]).
//! 3. The words ([`parse`]). A flag a form doesn't take, a verb `sessions`
//!    doesn't have and a missing id are each one `usage:` line, exit 2.
//! 4. One `_sidecar_send GET <route> <query>`
//!    ([`crate::transport::Client::sidecar_get`]): the cell socket with the
//!    passport when there is a live one, else (or when it failed, said "not
//!    served" or gave no status) TCP with `token=`. The workspace is
//!    `--workspace`, else the project, escaped as `urlencode` does.
//! 5. `_sessions_render` ([`render`]): a status that is not 200 is
//!    `daemon_too_old` (a 404 that names no route) or `_sidecar_fail`'s
//!    `code: hint` line with the exit for the status (400 and 413: 2, 403:
//!    3, 404: 4, else 1) and the refusal's closing line; `--json` prints
//!    the body as it came; else the shell's Python lays it out
//!    ([`roster`], [`past`], [`show`], [`main`]).
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]): only a failed
//!    call's body is noted, as `_sidecar_fail` notes it.
//!
//! The shell's call holds its answer in `$(…)` with curl's `-w
//! '\n%{http_code}'`, and every try of `_k2_curl` writes its own status:
//! with nothing listening the four tries leave `\n000` three times in the
//! body, and the shell prints them in its `daemon_error:` line. This
//! prints the same.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`):
//!
//! - `open`, `new`, `stop`, `main set`, `compact` and `set-label` (they
//!   change something), `archives` and `live` (the owner's on-disk and raw
//!   views, for a later slice), `spawn` (which skips the connection gate),
//!   `help`, and a command line with `-h` or `--help` ([`crate::front`]);
//! - a value flag as the last word (the shell's `shift 2` fails there and
//!   `set -e` ends the command), and a word that is not UTF-8;
//! - `HOOK_SOCK` set with a `CODEX_` variable exported, a port that is set
//!   but is no port curl takes, and no `python3` on `PATH`, as for
//!   [`super::inbox`].
//!
//! Where the shell's Python raises (an answer that is not JSON, a field
//! of a type it doesn't expect), this prints a Python 3.12 traceback: its
//! last line is Python's, its frames may differ by version, and for an
//! answer that is not JSON Python's has frames in its `json` module's
//! files and its own wording of where the text stopped being JSON. Bytes
//! that are not UTF-8 and half of a surrogate pair are as
//! [`super::inbox`] says. An answer cut short after its head (curl's 18)
//! is no answer here (`000`), where the shell reads what came.

use super::{
    curl_port, get, py_d, py_get, py_iter, py_left, py_len, py_space, raised, s, Raise, Shown,
};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::transport::encode::quote;
use crate::transport::{permission, Channel, Error, Response};
use std::ffi::OsString;
use std::path::Path;

/// The routes.
pub const ROSTER: &str = "/cli/sessions/roster";
pub const PAST: &str = "/cli/sessions/past";
pub const SHOW: &str = "/cli/sessions/show";
pub const MAIN: &str = "/cli/sessions/main";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "sessions-spawn";
/// `main` with a word it doesn't take.
pub const MAIN_USAGE: &str =
    "usage: k2 sessions main [--json]  |  k2 sessions main set <id> [--json]";
/// `main --thread` with a value it doesn't take.
pub const THREAD_USAGE: &str =
    "usage: --thread keep-main|adopt-sidecar (which Thread the main chat shows afterwards)";
/// `show` with a second id.
pub const SHOW_USAGE: &str = "usage: k2 sessions show <id|address> [--tail N] [--json]";
/// `show` with no id.
pub const SHOW_NO_ID: &str =
    "usage: k2 sessions show <id|address> [--tail N] [--json]  (ids: k2 sessions --past)";
/// A 404 that names no route.
pub const TOO_OLD: &str = concat!(
    "daemon_too_old: this ",
    k2_client::product_name!(),
    " daemon has no k2 sessions routes yet (update ",
    k2_client::product_name!(),
    ": k2 update --app)"
);

/// A flag the form doesn't take.
pub fn unknown_flag(word: &str) -> String {
    format!("usage: unknown flag '{word}' (k2 sessions --help)")
}

/// A verb `sessions` doesn't have.
pub fn unknown_verb(word: &str) -> String {
    format!("usage: unknown sessions verb '{word}' (k2 sessions --help)")
}

/// `_sidecar_fail`'s line when the body gave none.
pub fn unreachable(status: &str) -> String {
    format!(
        concat!(
            "daemon_unreachable: no answer from the ",
            k2_client::product_name!(),
            " daemon (status {})"
        ),
        status
    )
}

/// What the words ask for. An empty workspace is the project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Roster {
        workspace: String,
        all: bool,
        json: bool,
    },
    Past {
        workspace: String,
        search: String,
        days: String,
        cli: String,
        limit: String,
        archived: bool,
        json: bool,
    },
    Show {
        workspace: String,
        id: String,
        tail: String,
        json: bool,
    },
    Main {
        workspace: String,
        json: bool,
    },
}

impl Ask {
    fn json(&self) -> bool {
        match self {
            Ask::Roster { json, .. }
            | Ask::Past { json, .. }
            | Ask::Show { json, .. }
            | Ask::Main { json, .. } => *json,
        }
    }
}

/// The words after `sessions`, as `cmd_sessions` and its verbs read them:
/// the ask, or the `usage:` line that stops it (exit 2). `None`: the
/// shell's (see the module's notes).
pub fn parse(words: &[&str]) -> Option<Result<Ask, String>> {
    let Some(sub) = words.first() else {
        return roster(&[], false);
    };
    match *sub {
        "help" | "--help" | "-h" => return None,
        w if w.is_empty() || w.starts_with('-') => return roster(words, false),
        _ => {}
    }
    let rest = &words[1..];
    if rest.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return None;
    }
    match *sub {
        "main" => main_words(rest),
        "list" | "ls" => roster(rest, false),
        "past" => roster(rest, true),
        "show" => show_words(rest),
        "open" | "new" | "stop" | "archives" | "spawn" | "live" | "compact" | "set-label" => None,
        other => Some(Err(unknown_verb(other))),
    }
}

/// The value after a flag: `None` when there is none (the shell's).
fn value<'a>(words: &[&'a str], i: usize) -> Option<&'a str> {
    words.get(i + 1).copied()
}

/// `cmd_sessions_roster`'s loop. `past`: `past` put `--past` first.
fn roster(words: &[&str], past: bool) -> Option<Result<Ask, String>> {
    let (mut workspace, mut all, mut json, mut past) = (String::new(), false, false, past);
    let (mut search, mut days, mut cli, mut limit, mut archived) = (
        String::new(),
        String::new(),
        String::new(),
        String::new(),
        false,
    );
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--all" | "-a" => all = true,
            "--json" => json = true,
            "--past" => past = true,
            "--archived" => {
                archived = true;
                past = true;
            }
            "--search" | "--days" | "--cli" | "--limit" | "--workspace" => {
                let v = value(words, i)?.to_string();
                match *w {
                    "--search" => search = v,
                    "--days" => days = v,
                    "--cli" => cli = v,
                    "--limit" => limit = v,
                    _ => workspace = v,
                }
                if *w != "--workspace" {
                    past = true;
                }
                i += 1;
            }
            "-h" | "--help" => return None,
            other => return Some(Err(unknown_flag(other))),
        }
        i += 1;
    }
    Some(Ok(if past {
        Ask::Past {
            workspace,
            search,
            days,
            cli,
            limit,
            archived,
            json,
        }
    } else {
        Ask::Roster {
            workspace,
            all,
            json,
        }
    }))
}

/// `cmd_sessions_main`'s loop. `main set` is the shell's.
fn main_words(words: &[&str]) -> Option<Result<Ask, String>> {
    let (mut workspace, mut json, mut set, mut id) = (String::new(), false, false, String::new());
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--workspace" => {
                workspace = value(words, i)?.to_string();
                i += 1;
            }
            "--json" => json = true,
            "--thread" => {
                let t = value(words, i).unwrap_or("");
                if !matches!(
                    t,
                    "keep-main" | "keep_main" | "adopt-sidecar" | "adopt_sidecar"
                ) {
                    return Some(Err(THREAD_USAGE.to_string()));
                }
                i += 1;
            }
            "set" if !set => set = true,
            "set" => id = "set".into(),
            other if other.starts_with('-') => return Some(Err(unknown_flag(other))),
            other if set && id.is_empty() => id = other.to_string(),
            _ => return Some(Err(MAIN_USAGE.to_string())),
        }
        i += 1;
    }
    if set {
        return None;
    }
    Some(Ok(Ask::Main { workspace, json }))
}

/// `cmd_sessions_show`'s loop.
fn show_words(words: &[&str]) -> Option<Result<Ask, String>> {
    let (mut workspace, mut id, mut tail, mut json) =
        (String::new(), String::new(), String::new(), false);
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--tail" | "--workspace" => {
                let v = value(words, i)?.to_string();
                if *w == "--tail" {
                    tail = v;
                } else {
                    workspace = v;
                }
                i += 1;
            }
            "--json" => json = true,
            other if other.starts_with('-') => return Some(Err(unknown_flag(other))),
            other if id.is_empty() => id = other.to_string(),
            _ => return Some(Err(SHOW_USAGE.to_string())),
        }
        i += 1;
    }
    if id.is_empty() {
        return Some(Err(SHOW_NO_ID.to_string()));
    }
    Some(Ok(Ask::Show {
        workspace,
        id,
        tail,
        json,
    }))
}

/// The route and the query, for `project` when no workspace is named.
pub fn request(ask: &Ask, project: &str) -> (&'static str, String) {
    let ws = |w: &str| quote(if w.is_empty() { project } else { w });
    let mut q = String::from("workspace=");
    match ask {
        Ask::Roster { workspace, all, .. } => {
            q.push_str(&ws(workspace));
            if *all {
                q.push_str("&all=1");
            }
            (ROSTER, q)
        }
        Ask::Past {
            workspace,
            search,
            days,
            cli,
            limit,
            archived,
            ..
        } => {
            q.push_str(&ws(workspace));
            for (key, v) in [
                ("search", search),
                ("days", days),
                ("cli", cli),
                ("limit", limit),
            ] {
                if !v.is_empty() {
                    q.push_str(&format!("&{key}={}", quote(v)));
                }
            }
            if *archived {
                q.push_str("&archived=1");
            }
            (PAST, q)
        }
        Ask::Show {
            workspace,
            id,
            tail,
            ..
        } => {
            q.push_str(&ws(workspace));
            q.push_str("&id=");
            q.push_str(&quote(id));
            if !tail.is_empty() {
                q.push_str("&tail=");
                q.push_str(&quote(tail));
            }
            (SHOW, q)
        }
        Ask::Main { workspace, .. } => {
            q.push_str(&ws(workspace));
            (MAIN, q)
        }
    }
}

/// `k5 sessions ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
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
    let ask = parse(&words)?;
    if !super::has_python3() {
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
    Some(run_with(args, ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    ask: Result<Ask, String>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(notice) = prelude::rename_notice(Path::new(&settings.project)) {
        out::stderr_line(notice);
    }
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let ask = match ask {
        Ok(a) => a,
        Err(line) => {
            out::stderr_line(&line);
            return Exit::Usage;
        }
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
    let (route, q) = request(&ask, &settings.project);
    let (status, body) = held(client.sidecar_get(route, &q));
    let mut closing = Closing::default();
    let exit = if status != "200" {
        if status == "404" && (contains(&body, "route not found") || contains(&body, "Not Found")) {
            out::stderr_line(TOO_OLD);
            Exit::Error
        } else {
            out::stderr_line(&fail_line(&status, &body));
            let r = Response {
                status: status.parse().unwrap_or(0),
                permission: permission::note(&body),
                body,
                channel: Channel::Tcp,
                server_version: None,
            };
            closing.answer(&r, client.passport().is_some());
            exit_for(&status)
        }
    } else if ask.json() {
        let mut text = body;
        text.push(b'\n');
        match out::write_flushed(&mut std::io::stdout().lock(), &text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        }
    } else {
        render(&ask, &body, now()).print()
    };
    closing.flush(&mut client, exit);
    exit
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// `_sidecar_split` on what `raw="$(_sidecar_send …)"` holds: the status
/// (`000` for none) and the body, NULs dropped. Every `_k2_curl` try
/// that found nothing listening wrote `\n000`: the last is the status,
/// the others stay in the body.
pub fn held(r: Result<Response, Error>) -> (String, Vec<u8>) {
    match r {
        Ok(r) => (format!("{:03}", r.status), super::without_nuls(&r.body)),
        Err(Error::Unreachable { .. }) => (
            "000".into(),
            "\n000"
                .repeat(crate::transport::client::TRIES as usize - 1)
                .into_bytes(),
        ),
        Err(_) => ("000".into(), Vec::new()),
    }
}

/// `_sidecar_exit_for`.
pub fn exit_for(status: &str) -> Exit {
    match status {
        "400" | "413" => Exit::Usage,
        "403" => Exit::Refused,
        "404" => Exit::NotFound,
        _ => Exit::Error,
    }
}

/// `_sidecar_fail`'s line: its Python's on the body (`$(…)` drops the
/// trailing newlines), or the no-answer line when that is empty.
pub fn fail_line(status: &str, body: &[u8]) -> String {
    let mut line = String::new();
    if !body.is_empty() {
        let raw = String::from_utf8_lossy(body);
        line = match json::parse(&raw) {
            Err(_) => format!(
                "daemon_error: {}",
                raw.chars().take(200).collect::<String>()
            ),
            Ok(v) => {
                let e = match &v {
                    Value::Object(pairs) => get(pairs, "error"),
                    _ => None,
                };
                match e {
                    Some(Value::Object(e)) => format!(
                        "{}: {}",
                        get(e, "code").map_or_else(|| "error".into(), py::str_of),
                        get(e, "hint").map_or_else(String::new, py::str_of)
                    ),
                    Some(e) if py::truthy(e) => format!("error: {}", py::str_of(e)),
                    _ => "error: unexpected response".into(),
                }
            }
        };
        while line.ends_with('\n') {
            line.pop();
        }
    }
    if line.is_empty() {
        unreachable(status)
    } else {
        line
    }
}

/// `time.time()`.
fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// The lines of the shell's Python that raise or call (`tests` pins them).
pub mod line {
    pub const LOAD: u32 = 4;
    pub const AGO_D: u32 = 9;
    pub const CUT_S: u32 = 15;
    pub const R_ROWS: u32 = 18;
    pub const R_WS: u32 = 20;
    pub const R_LIVE: u32 = 21;
    pub const R_HEAD: u32 = 22;
    pub const R_TITLE: u32 = 28;
    pub const R_AGO: u32 = 30;
    pub const R_ASLEEP: u32 = 39;
    pub const P_ROWS: u32 = 43;
    pub const P_DAYS: u32 = 45;
    pub const P_FOR: u32 = 56;
    pub const P_RUN: u32 = 57;
    pub const P_NOW: u32 = 59;
    pub const P_ROW: u32 = 63;
    pub const P_MATCH: u32 = 65;
    pub const P_FIRST: u32 = 68;
    pub const P_FILES: u32 = 73;
    pub const P_UNSUPPORTED: u32 = 74;
    pub const P_NOTE: u32 = 75;
    pub const S_C: u32 = 77;
    pub const S_WHERE: u32 = 78;
    pub const S_HEAD: u32 = 79;
    pub const S_COUNT: u32 = 81;
    pub const S_ASK: u32 = 91;
    pub const S_LEN: u32 = 93;
    pub const S_FOR: u32 = 94;
    pub const S_MSG: u32 = 95;
    pub const S_TOOLS: u32 = 97;
    pub const S_RUN: u32 = 98;
    pub const M_M: u32 = 112;
    pub const M_STATE: u32 = 114;
    pub const M_PATH: u32 = 127;
}

/// The shell's Python on a 200's body, for the ask's verb. `now`: its
/// `time.time()`.
pub fn render(ask: &Ask, body: &[u8], now: f64) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let v = match json::parse(&raw) {
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
                &Raise::at(line::LOAD, format!("json.decoder.JSONDecodeError: {why}")),
            );
        }
    };
    let mut o = Out::default();
    let r = match ask {
        Ask::Roster { .. } => roster_lines(&v, now, &mut o),
        Ask::Past { .. } => past_lines(&v, now, &mut o),
        Ask::Show { .. } => show_lines(&v, now, &mut o),
        Ask::Main { .. } => main_lines(&v, &mut o),
    };
    match r {
        Ok(exit) => Shown {
            stdout: o.stdout,
            stderr: o.stderr,
            exit,
        },
        Err(r) => raised(o.stdout, o.stderr, &r),
    }
}

#[derive(Default)]
struct Out {
    stdout: String,
    stderr: String,
}

impl Out {
    fn line(&mut self, text: &str) {
        self.stdout.push_str(text);
        self.stdout.push('\n');
    }
}

/// `x or default`.
fn or(v: Option<&Value>, default: Value) -> Value {
    v.filter(|x| py::truthy(x)).cloned().unwrap_or(default)
}

/// `d.get(key)` where `d` is a dict for sure.
fn at<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    match d {
        Value::Object(pairs) => get(pairs, key),
        _ => None,
    }
}

/// `str(x)` of `d.get(key)`, `None` for a missing key.
fn str_at(d: &Value, key: &str) -> String {
    at(d, key).map_or_else(|| "None".into(), py::str_of)
}

/// The shell's `ago(t)`, called from `line`.
pub fn ago(t: Option<&Value>, now: f64, from: u32) -> Result<String, Raise> {
    let Some(t) = t.filter(|t| py::truthy(t)) else {
        return Ok("-".into());
    };
    let inside = |exception: String| Raise::inside(from, line::AGO_D, "ago", exception);
    let t = match t {
        Value::Int(digits) => {
            let f: f64 = digits.parse().unwrap_or(f64::INFINITY);
            if f.is_infinite() {
                return Err(inside(
                    "OverflowError: int too large to convert to float".into(),
                ));
            }
            f
        }
        Value::Float(f) => *f,
        Value::Bool(_) => 1.0,
        other => {
            return Err(inside(format!(
                "TypeError: unsupported operand type(s) for -: 'float' and '{}'",
                py::type_name(other)
            )))
        }
    };
    let x = now - t;
    if x.is_nan() {
        return Err(inside(
            "ValueError: cannot convert float NaN to integer".into(),
        ));
    }
    if x.is_infinite() {
        return Err(inside(
            "OverflowError: cannot convert float infinity to integer".into(),
        ));
    }
    let d = x.trunc().max(0.0);
    let n = |div: f64| py::percent_d(&Value::Float((d / div).floor())).unwrap_or_default();
    Ok(if d < 60.0 {
        "now".into()
    } else if d < 3600.0 {
        format!("{}m", n(60.0))
    } else if d < 86400.0 {
        format!("{}h", n(3600.0))
    } else {
        format!("{}d", n(86400.0))
    })
}

/// The shell's `cut(s, n)`, called from `line`.
pub fn cut(v: Option<&Value>, n: usize, from: u32) -> Result<String, Raise> {
    let text = match v.filter(|x| py::truthy(x)) {
        None => String::new(),
        Some(t @ Value::Str(_)) => py::str_of(t),
        Some(other) => {
            return Err(Raise::inside(
                from,
                line::CUT_S,
                "cut",
                format!(
                    "AttributeError: '{}' object has no attribute 'replace'",
                    py::type_name(other)
                ),
            ))
        }
    };
    let s = text.replace('\n', " ");
    let s = s.trim_matches(py_space);
    if s.chars().count() <= n {
        Ok(s.to_string())
    } else {
        Ok(format!(
            "{}\u{2026}",
            s.chars().take(n.saturating_sub(1)).collect::<String>()
        ))
    }
}

/// `verb == "roster"`.
fn roster_lines(v: &Value, now: f64, o: &mut Out) -> Result<Exit, Raise> {
    let empty = Value::Array(Vec::new());
    let rows = py_get(v, "sessions", line::R_ROWS)?
        .cloned()
        .unwrap_or_else(|| empty.clone());
    let sc = or(at(v, "sidecars"), Value::Object(Vec::new()));
    let wsv = or(at(v, "workspace"), Value::Object(Vec::new()));
    let ws = or(py_get(&wsv, "handle", line::R_WS)?, s("?"));
    let items = py_iter(&rows, line::R_LIVE)?;
    let mut live = 0;
    for r in &items {
        if py_get(r, "state", line::R_LIVE)? != Some(&s("asleep")) {
            live += 1;
        }
    }
    let cap = py_get(&sc, "cap", line::R_HEAD)?
        .cloned()
        .unwrap_or(Value::Int("0".into()));
    let in_use = at(&sc, "live").cloned().unwrap_or(Value::Int("0".into()));
    let cap = py_d(&cap, line::R_HEAD)?;
    let in_use = py_d(&in_use, line::R_HEAD)?;
    o.line(&format!(
        "{} \u{2014} {live} live (cap {cap} sidecars; {in_use} in use)",
        py::str_of(&ws)
    ));
    let fmt = |c: [&str; 7]| {
        format!(
            "{} {} {} {} {} {} {}",
            c[0],
            super::ljust(c[1], 16),
            super::ljust(c[2], 9),
            super::ljust(c[3], 7),
            super::ljust(c[4], 8),
            super::ljust(c[5], 7),
            c[6]
        )
    };
    if py::truthy(&rows) {
        o.line(&fmt([
            " ",
            "ADDRESS",
            "ROLE",
            "CLI",
            "STATE",
            "ACTIVE",
            "TITLE / BRIEF",
        ]));
        for r in &items {
            let addr = or(
                at(r, "address")
                    .filter(|x| py::truthy(x))
                    .or(at(r, "handle")),
                s("-"),
            );
            let title = cut(at(r, "title"), 60, line::R_TITLE)?;
            let you = if at(r, "you").is_some_and(py::truthy) {
                "*"
            } else {
                " "
            };
            let role = py::str_of(&or(at(r, "role"), s("-")));
            let cli = py::str_of(&or(at(r, "cli"), s("-")));
            let state = py::str_of(&or(at(r, "state"), s("-")));
            let active = ago(at(r, "lastActiveAt"), now, line::R_AGO)?;
            o.line(&fmt([
                you,
                &py::str_of(&addr),
                &role,
                &cli,
                &state,
                &active,
                &title,
            ]));
        }
        let example = items.iter().find_map(|r| {
            at(r, "address")
                .filter(|a| py::truthy(a) && !at(r, "you").is_some_and(py::truthy))
                .cloned()
        });
        let hint = match example {
            Some(e) => format!("Message one: k2 msg {} \"\u{2026}\"", py::str_of(&e)),
            None => "Message one: k2 msg <address> \"\u{2026}\"".into(),
        };
        let you = items.iter().any(|r| at(r, "you").is_some_and(py::truthy));
        o.line(&format!("{}{hint}", if you { "* = you.  " } else { "" }));
    } else {
        o.line("(nobody is running here)");
    }
    let asleep = at(v, "asleep").cloned().unwrap_or(Value::Int("0".into()));
    let mut tail = Vec::new();
    if py::truthy(&asleep) && !items.iter().any(|r| at(r, "state") == Some(&s("asleep"))) {
        tail.push(format!(
            "{} asleep (k2 sessions --all)",
            py_d(&asleep, line::R_ASLEEP)?
        ));
    }
    tail.push("past conversations: k2 sessions --past".into());
    o.line(&tail.join(" \u{b7} "));
    Ok(Exit::Ok)
}

/// `verb == "past"`.
fn past_lines(v: &Value, now: f64, o: &mut Out) -> Result<Exit, Raise> {
    let rows = py_get(v, "conversations", line::P_ROWS)?
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    let ws = or(at(v, "workspace"), s("?"));
    let days = at(v, "days").cloned().unwrap_or(Value::Int("14".into()));
    let mut what = format!("last {} days", py_d(&days, line::P_DAYS)?);
    if let Some(search) = at(v, "search").filter(|x| py::truthy(x)) {
        what.push_str(&format!(", matching \"{}\"", py::str_of(search)));
    }
    if let Some(cli) = at(v, "cli").filter(|x| py::truthy(x)) {
        what.push_str(&format!(", {} only", py::str_of(cli)));
    }
    o.line(&format!(
        "Past conversations in {} ({what}):",
        py::str_of(&ws)
    ));
    if !py::truthy(&rows) {
        o.line("  (none found; try --days 90, or another --search)");
    } else {
        let fmt = |c: [&str; 6]| {
            format!(
                "  {} {} {} {} {} {}",
                super::ljust(c[0], 10),
                super::ljust(c[1], 7),
                super::ljust(c[2], 5),
                super::ljust(c[3], 24),
                super::ljust(c[4], 34),
                c[5]
            )
        };
        o.line(&fmt(["ID", "CLI", "LAST", "WHERE", "TITLE", "NOW"]));
        let items = py_iter(&rows, line::P_FOR)?;
        for r in &items {
            let run = or(py_get(r, "runner", line::P_RUN)?, Value::Object(Vec::new()));
            let now_s = if py::truthy(&run) {
                let state = or(py_get(&run, "state", line::P_NOW)?, s("live"));
                let addr = or(at(&run, "address"), s("-"));
                format!("{} as {}", py::str_of(&state), py::str_of(&addr))
            } else {
                "-".into()
            };
            let title = at(r, "customName")
                .filter(|x| py::truthy(x))
                .or(at(r, "title"))
                .cloned();
            let last = ago(at(r, "lastActiveAt"), now, line::P_ROW)?;
            let worktree = or(at(r, "worktree"), s("(root)"));
            let where_ = cut(Some(&worktree), 24, line::P_ROW)?;
            let title = cut(title.as_ref(), 34, line::P_ROW)?;
            o.line(&format!(
                "  {} {} {} {} {} {now_s}",
                py_left(&at(r, "id").cloned().unwrap_or(Value::Null), 10),
                py_left(&at(r, "cli").cloned().unwrap_or(Value::Null), 7),
                super::ljust(&last, 5),
                super::ljust(&where_, 24),
                super::ljust(&title, 34),
            ));
            if let Some(m) = at(r, "match").filter(|x| py::truthy(x)) {
                o.line(&format!(
                    "             match: {}",
                    cut(Some(m), 110, line::P_MATCH)?
                ));
            }
        }
        let first = items
            .iter()
            .find(|r| !at(r, "runner").is_some_and(py::truthy));
        match first {
            Some(f) if py::truthy(f) => {
                let Some(id) = at(f, "id") else {
                    return Err(Raise::at(line::P_FIRST, "KeyError: 'id'"));
                };
                let id = py::str_of(id);
                o.line(&format!("Open one: k2 sessions open {id} --name <name>   \u{b7}   Read first: k2 sessions show {id}"));
            }
            _ => o.line("Every one listed is held already: k2 msg its address (NOW column)."),
        }
    }
    if at(v, "truncated").is_some_and(py::truthy) {
        let sc = or(at(v, "scanned"), Value::Object(Vec::new()));
        let files = py_get(&sc, "files", line::P_FILES)?
            .cloned()
            .unwrap_or(Value::Int("0".into()));
        o.line(&format!(
            "(search stopped after {} files; narrow with --days or --cli)",
            py_d(&files, line::P_FILES)?
        ));
    }
    let unsupported = or(at(v, "unsupported"), Value::Array(Vec::new()));
    for u in py_iter(&unsupported, line::P_UNSUPPORTED)? {
        let cli = py_get(&u, "cli", line::P_NOTE)?
            .cloned()
            .unwrap_or(Value::Null);
        let why = at(&u, "why").cloned().unwrap_or(Value::Null);
        o.stderr.push_str(&format!(
            "note: {}: {}\n",
            py::str_of(&cli),
            py::str_of(&why)
        ));
    }
    Ok(Exit::Ok)
}

/// `verb == "show"`.
fn show_lines(v: &Value, now: f64, o: &mut Out) -> Result<Exit, Raise> {
    let c = or(
        py_get(v, "conversation", line::S_C)?,
        Value::Object(Vec::new()),
    );
    let where_ = or(py_get(&c, "worktree", line::S_WHERE)?, s("root"));
    let title = at(&c, "customName")
        .filter(|x| py::truthy(x))
        .or(at(&c, "title"))
        .cloned();
    let title = cut(title.as_ref(), 60, line::S_HEAD)?;
    let last = ago(at(&c, "lastActiveAt"), now, line::S_HEAD)?;
    let mut head = format!(
        "{}  {}  \"{title}\"   last active {last} ago",
        str_at(&c, "id"),
        str_at(&c, "cli")
    );
    if let Some(n) = at(v, "messageCount").filter(|x| **x != Value::Null) {
        head.push_str(&format!(" \u{b7} {} messages", py_d(n, line::S_COUNT)?));
    }
    head.push_str(&format!(" \u{b7} {}", py::str_of(&where_)));
    if let Some(b) = at(&c, "branch").filter(|x| py::truthy(x)) {
        head.push_str(&format!(" \u{b7} branch {}", py::str_of(b)));
    }
    o.line(&head);
    let run = at(&c, "runner").cloned().unwrap_or(Value::Null);
    if let Some(u) = at(v, "unsupported").filter(|x| py::truthy(x)) {
        o.line(&format!("({})", py::str_of(u)));
    } else {
        if let Some(a) = at(v, "firstAsk").filter(|x| py::truthy(x)) {
            o.line(&format!(
                "Asked first: \"{}\"",
                cut(Some(a), 200, line::S_ASK)?
            ));
        }
        let msgs = or(at(v, "messages"), Value::Array(Vec::new()));
        o.line(&format!(
            "--- last {} messages ---",
            py_len(&msgs, line::S_LEN)?
        ));
        for m in py_iter(&msgs, line::S_FOR)? {
            let role = py_get(&m, "role", line::S_MSG)?
                .cloned()
                .unwrap_or(Value::Null);
            let when = match at(&m, "at").filter(|x| py::truthy(x)) {
                Some(t) => ago(Some(t), now, line::S_MSG)?,
                None => String::new(),
            };
            let text = at(&m, "text").cloned().unwrap_or_else(|| s(""));
            o.line(&format!(
                "[{} {when}]  {}",
                py::str_of(&role),
                py::str_of(&text)
            ));
        }
        if let Some(t) = at(v, "toolCalls").filter(|x| py::truthy(x)) {
            o.line(&format!(
                "({} tool calls not shown)",
                py_d(t, line::S_TOOLS)?
            ));
        }
    }
    let address = if py::truthy(&run) {
        py_get(&run, "address", line::S_RUN)?.filter(|a| py::truthy(a))
    } else {
        None
    };
    match address {
        Some(a) => {
            let a = py::str_of(a);
            o.line(&format!(
                "It is {} as {a}. Message it: k2 msg {a} \"\u{2026}\"",
                str_at(&run, "state")
            ));
        }
        None => {
            let id = str_at(&c, "id");
            o.line(&format!(
                "Reopen: k2 sessions open {id}   (or k2 msg --session {id} \"\u{2026}\")"
            ));
        }
    }
    Ok(Exit::Ok)
}

/// `verb == "main"`.
fn main_lines(v: &Value, o: &mut Out) -> Result<Exit, Raise> {
    let m = or(py_get(v, "main", line::M_M)?, Value::Object(Vec::new()));
    let ws = py::str_of(&or(at(v, "workspace"), s("?")));
    let state = py_get(&m, "state", line::M_STATE)?;
    let is = |x: &str| state == Some(&s(x));
    let (id, cli) = (str_at(&m, "conversationId"), str_at(&m, "cli"));
    if is("none") {
        o.line(&format!(
            "{ws} has no main chat pinned yet (it starts on the next message)."
        ));
    } else if is("missing") {
        o.stderr.push_str(&format!("{ws} main chat {id} ({cli}) CANNOT BE FOUND: no conversation on disk in any of its folders.\n"));
        o.stderr.push_str(concat!(
            k2_client::product_name!(),
            " kept it pinned and did not start or switch to another chat.\n"
        ));
        o.stderr
            .push_str("Choose one: k2 sessions --past, then k2 sessions main set <id>\n");
        return Ok(Exit::Error);
    } else if is("unstarted") {
        o.line(&format!(
            "{ws} main chat {id} ({cli}): pre-allocated, no first turn yet."
        ));
    } else if is("custom") {
        o.line(&format!(
            concat!(
                "{} main chat {} ({}): a custom preset; ",
                k2_client::product_name!(),
                " cannot check it on disk."
            ),
            ws, id, cli
        ));
    } else {
        let path = match at(&m, "path").filter(|x| py::truthy(x)) {
            None => String::new(),
            Some(p @ Value::Str(_)) => format!("\n  {}", py::str_of(p)),
            Some(other) => {
                return Err(Raise::at(
                    line::M_PATH,
                    format!(
                        "TypeError: can only concatenate str (not \"{}\") to str",
                        py::type_name(other)
                    ),
                ))
            }
        };
        o.line(&format!("{ws} main chat {id} ({cli}){path}"));
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests;
