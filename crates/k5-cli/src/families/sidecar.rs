//! `k5 sidecar`, as the shell CLI's `cmd_sidecar` answers it
//! (`cli/src/sidecar.sh`): `new`, `list` (`ls`), `stop`, `archive`,
//! `interrupt`, `review`, `land`, `retire` and `access on|off|status`,
//! each with `--json` where the shell takes it, and the help.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at
//!    once, the virtual desktop's guard (not with `-h` or `--help`
//!    anywhere) and the connection gate (not for the help: `sidecar`
//!    alone, `help`, `--help`, `-h`, or `-h`/`--help` anywhere after the
//!    verb).
//! 2. `sidecar` is an `id` tool: with `HOOK_SOCK` set, the stale-passport
//!    or require-passport lines, exit 3, the help too.
//! 3. The verb's words ([`parse_new`], [`name_args`] and the rest): each
//!    problem is one `usage:` line, exit 2. A value flag as the last word
//!    is the shell's `_need_value` line, exit 2 ([`Stop::Shift`]).
//! 4. `_sidecar_send` ([`crate::confirm::sidecar_call`]): the cell socket
//!    with the passport when there is a live one, else TCP. A POST that
//!    went out on the socket is never sent again.
//! 5. `_sidecar_render`: a status that is not 200 is `_sidecar_fail`'s
//!    `code: hint` line ([`super::sessions::fail_line`]) with its exit and
//!    the refusal's closing line; `--json` prints the body as it came;
//!    else the shell's Python lays it out ([`Reader`]).
//! 6. `land`, and `retire --discard`, first read the review and show it,
//!    then ask on the terminal (`--yes` skips it; with no terminal the
//!    command stops, exit 2), then send.
//!
//! The shell CLI answers these itself ([`run`] returns `None`): a word
//! that is not UTF-8, `HOOK_SOCK` set with a `CODEX_` variable exported,
//! a port that is set but is no port curl takes (for a form that asks), a
//! `python3` the binary can't tell, and `new --brief -` where `mktemp`
//! has no temp folder to write in.
//!
//! Where the shell's Python raises (an answer that is not JSON, a field
//! of a type it doesn't expect), this prints a Python 3.12 traceback, as
//! [`super::sessions`] says.

use super::sessions::{exit_for, fail_line};
use super::{curl_port, py_d, py_get, py_iter, py_left, py_len, raised, s, Raise, Shown};
use crate::confirm::{self, Split};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::Value;
use crate::out::pylib::{json as pyjson, ops, time};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::prompt;
use crate::python;
use crate::transport::encode::quote;
use crate::transport::Client;
use k2_client::brand::CLI_NAME as CLI;
use std::ffi::OsString;
use std::io::Read;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["sidecar"];
/// The binary answers every form ([`run`] may still hand one to the
/// shell CLI).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: the whole row is native.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "sidecar";

/// Why the words stop before any request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// One `usage:` line, exit 2.
    Usage(String),
    /// A value flag as the last word: the shell's `_need_value` line
    /// ([`crate::out::need_value`]), exit 2.
    Shift(String),
}

/// A flag the verb doesn't have.
pub fn unknown_flag(flag: &str) -> String {
    format!("usage: unknown flag '{flag}' ({CLI} sidecar --help)")
}

/// A verb `sidecar` doesn't have.
pub fn unknown_verb(sub: &str) -> String {
    format!(
        "usage: unknown sidecar verb '{sub}' (new|list|stop|archive|interrupt|review|land|retire|access \u{2014} {CLI} sidecar --help)"
    )
}

/// `new` with no name or no harness.
pub fn new_usage() -> String {
    format!(
        "usage: {CLI} sidecar new <name> --harness <preset> [--brief <file>|-] [--brief-text \"\u{2026}\"] [--worktree [--base <ref>]]"
    )
}

/// `list` with a word it doesn't take.
pub fn list_usage() -> String {
    format!("usage: {CLI} sidecar list [--workspace <ws>] [--json]")
}

/// `archive` with neither a name nor `--all-stopped`, or both.
pub fn archive_usage() -> String {
    format!("usage: {CLI} sidecar archive <name>|--all-stopped [--workspace <ws>] [--json]")
}

/// `stop` with no name, or two.
pub fn stop_usage() -> String {
    format!("usage: {CLI} sidecar stop <name> [--workspace <ws>] [--force] [--json]")
}

/// `access` with a word it doesn't take.
pub fn access_usage() -> String {
    format!("usage: {CLI} sidecar access on|off|status [--workspace <ws>]")
}

/// The usage the Workbench verbs name (without `usage: `).
pub fn verb_usage(verb: &str) -> String {
    let flags = match verb {
        "land" => " [--keep] [--yes]",
        "retire" => " [--discard] [--keep-branch] [--yes]",
        _ => "",
    };
    format!("{CLI} sidecar {verb} <name>{flags} [--workspace <ws>] [--json]")
}

/// `_sidecar_confirm` with no terminal.
pub const NO_TERMINAL: &str =
    "not done: no terminal to ask on; read the above, then run it again with --yes";

/// What `cmd_sidecar` does with its words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `_sidecar_usage`: the help, exit 0.
    Help,
    /// A verb and the words after it.
    Verb(String, Vec<String>),
}

/// `cmd_sidecar`'s first look: the help, else the verb.
pub fn parse(words: &[&str]) -> Parsed {
    let sub = words.first().copied().unwrap_or("");
    if matches!(sub, "" | "help" | "--help" | "-h")
        || words[1..].iter().any(|w| matches!(*w, "-h" | "--help"))
    {
        return Parsed::Help;
    }
    Parsed::Verb(
        sub.to_string(),
        words[1..].iter().map(|w| w.to_string()).collect(),
    )
}

/// `cmd_sidecar_new`'s words.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct New {
    pub name: String,
    pub harness: String,
    pub brief_file: String,
    pub brief_text: String,
    pub have_text: bool,
    pub workspace: String,
    pub model: String,
    pub json: bool,
    pub worktree: bool,
    pub base: String,
}

/// `cmd_sidecar_new`'s loop and checks, up to the brief file.
pub fn parse_new(words: &[String]) -> Result<New, Stop> {
    let mut n = New::default();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--worktree" => {
                n.worktree = true;
                if n.name.is_empty() {
                    if let Some(next) = words.get(i + 1) {
                        if !next.starts_with('-') {
                            n.name = next.clone();
                            i += 1;
                        }
                    }
                }
            }
            "--base" | "--harness" | "--brief" | "--brief-text" | "--workspace" | "--model" => {
                let v = words
                    .get(i + 1)
                    .ok_or_else(|| Stop::Shift(a.to_string()))?
                    .clone();
                match a {
                    "--base" => n.base = v,
                    "--harness" => n.harness = v,
                    "--brief" => n.brief_file = v,
                    "--brief-text" => {
                        n.brief_text = v;
                        n.have_text = true;
                    }
                    "--workspace" => n.workspace = v,
                    _ => n.model = v,
                }
                i += 1;
            }
            "--json" => n.json = true,
            "--" => break,
            _ if a.starts_with('-') => return Err(Stop::Usage(unknown_flag(a))),
            _ => {
                if n.name.is_empty() {
                    n.name = a.to_string();
                } else {
                    return Err(Stop::Usage(format!(
                        "usage: one name only; quote names with spaces ({CLI} sidecar --help)"
                    )));
                }
            }
        }
        i += 1;
    }
    if n.name.is_empty() || n.harness.is_empty() {
        return Err(Stop::Usage(new_usage()));
    }
    if !n.base.is_empty() && !n.worktree {
        return Err(Stop::Usage(format!(
            "usage: --base goes with --worktree ({CLI} sidecar --help)"
        )));
    }
    if !n.brief_file.is_empty() && n.have_text {
        return Err(Stop::Usage(
            "usage: pass --brief or --brief-text, not both".into(),
        ));
    }
    Ok(n)
}

impl New {
    /// The body's Python: `json.dumps(b)`, with the brief's text when
    /// there is one. `Err`: what the shell prints, and exit 2.
    pub fn body(
        &self,
        workspace: &str,
        brief: Option<Result<Vec<u8>, String>>,
    ) -> Result<String, String> {
        let mut b = vec![
            ("workspace", s(workspace)),
            ("name", s(&self.name)),
            ("harness", s(&self.harness)),
        ];
        if !self.model.is_empty() {
            b.push(("model", s(&self.model)));
        }
        if self.worktree {
            b.push(("worktree", Value::Bool(true)));
            if !self.base.is_empty() {
                b.push(("base", s(&self.base)));
            }
        }
        match brief {
            Some(Ok(raw)) => match String::from_utf8(raw) {
                Ok(text) => b.push(("brief", s(&text))),
                Err(_) => return Err("brief_not_text: the brief is not UTF-8 text\n".into()),
            },
            Some(Err(traceback)) => return Err(traceback),
            None if self.have_text => b.push(("brief", s(&self.brief_text))),
            None => {}
        }
        Ok(confirm::dumps(&confirm::object(b)))
    }
}

/// `_sidecar_name_args`: one name plus the Workbench verbs' flags.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Named {
    pub name: String,
    pub workspace: String,
    pub json: bool,
    pub yes: bool,
    pub keep: bool,
    pub discard: bool,
    pub keep_branch: bool,
}

/// `_sidecar_name_args "<usage>" "$@"`.
pub fn name_args(usage: &str, words: &[String]) -> Result<Named, Stop> {
    let mut n = Named::default();
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--workspace" => {
                n.workspace = words
                    .get(i + 1)
                    .ok_or_else(|| Stop::Shift(a.to_string()))?
                    .clone();
                i += 1;
            }
            "--json" => n.json = true,
            "--yes" | "-y" => n.yes = true,
            "--keep" => n.keep = true,
            "--discard" => n.discard = true,
            "--keep-branch" => n.keep_branch = true,
            _ if a.starts_with('-') => {
                return Err(Stop::Usage(format!("usage: unknown flag '{a}' ({usage})")))
            }
            _ => {
                if n.name.is_empty() {
                    n.name = a.to_string();
                } else {
                    return Err(Stop::Usage(format!("usage: {usage}")));
                }
            }
        }
        i += 1;
    }
    if n.name.is_empty() {
        return Err(Stop::Usage(format!("usage: {usage}")));
    }
    Ok(n)
}

/// `list`, `stop` and `archive`: a name, `--workspace`, `--json` and the
/// verb's own switch (`--force`, `--all-stopped`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Simple {
    pub name: String,
    pub workspace: String,
    pub json: bool,
    pub switch: bool,
}

/// `cmd_sidecar_list`, `cmd_sidecar_stop` and `cmd_sidecar_archive`'s
/// loops and checks.
pub fn parse_simple(verb: &str, words: &[String]) -> Result<Simple, Stop> {
    let mut n = Simple::default();
    let switch = match verb {
        "stop" => "--force",
        "archive" => "--all-stopped",
        _ => "",
    };
    let usage = match verb {
        "stop" => stop_usage(),
        "archive" => archive_usage(),
        _ => list_usage(),
    };
    let mut i = 0;
    while i < words.len() {
        let a = words[i].as_str();
        match a {
            "--workspace" => {
                n.workspace = words
                    .get(i + 1)
                    .ok_or_else(|| Stop::Shift(a.to_string()))?
                    .clone();
                i += 1;
            }
            "--json" => n.json = true,
            _ if !switch.is_empty() && a == switch => n.switch = true,
            _ if verb == "list" => return Err(Stop::Usage(usage)),
            _ if a.starts_with('-') => return Err(Stop::Usage(unknown_flag(a))),
            _ => {
                if n.name.is_empty() {
                    n.name = a.to_string();
                } else {
                    return Err(Stop::Usage(usage));
                }
            }
        }
        i += 1;
    }
    let bad = match verb {
        "stop" => n.name.is_empty(),
        "archive" => n.name.is_empty() != n.switch,
        _ => false,
    };
    if bad {
        return Err(Stop::Usage(usage));
    }
    Ok(n)
}

/// `cmd_sidecar_access`'s words: the action and the workspace.
pub fn parse_access(words: &[String]) -> Result<(String, String), Stop> {
    let action = words.first().cloned().unwrap_or_default();
    let rest = words.get(1..).unwrap_or_default();
    let mut workspace = String::new();
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == "--workspace" {
            workspace = rest
                .get(i + 1)
                .ok_or_else(|| Stop::Shift("--workspace".into()))?
                .clone();
            i += 2;
        } else {
            return Err(Stop::Usage(access_usage()));
        }
    }
    if !matches!(action.as_str(), "on" | "off" | "status") {
        return Err(Stop::Usage(access_usage()));
    }
    Ok((action, workspace))
}

/// Which of the shell's Python readers lays out a 200.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    New,
    Stop,
    Archive,
    List,
    Access,
    Interrupt,
    Review,
    Land,
    Retire,
}

/// Whether a command line skips the virtual desktop's guard: `-h` or
/// `--help` anywhere in it (`_k2_offline_verb`).
fn offline_verb(words: &[&str]) -> bool {
    words.iter().any(|w| matches!(*w, "-h" | "--help"))
}

/// `mktemp -t …` has a folder to write in: `$TMPDIR`, else `/tmp`.
#[cfg(unix)]
fn temp_folder_usable() -> bool {
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
fn temp_folder_usable() -> bool {
    true
}

/// `k5 sidecar ...`: `args` is the whole command line after `argv[0]`.
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
    if let Parsed::Verb(sub, rest) = &parsed {
        if sub == "new" {
            if let Ok(n) = parse_new(rest) {
                if n.brief_file == "-" && !temp_folder_usable() {
                    return None;
                }
            }
        }
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
    match prelude::gate(&settings) {
        Some(Gate::VirtualDesktop) if !offline => return Gate::VirtualDesktop.stop(),
        Some(_) if *parsed != Parsed::Help => return Gate::CannotConnect.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let (sub, rest) = match parsed {
        Parsed::Help => {
            return Shown::out(crate::text::help("sidecar").unwrap_or_default()).print()
        }
        Parsed::Verb(sub, rest) => (sub.as_str(), rest.as_slice()),
    };
    if !matches!(
        sub,
        "new"
            | "list"
            | "ls"
            | "stop"
            | "archive"
            | "access"
            | "interrupt"
            | "review"
            | "land"
            | "retire"
    ) {
        out::stderr_line(&unknown_verb(sub));
        return Exit::Usage;
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let project = settings.project.clone();
    let ws = |w: &str| {
        if w.is_empty() {
            project.clone()
        } else {
            w.to_string()
        }
    };
    let mut client = None;
    let mut closing = Closing::default();
    let exit = match verb(
        sub,
        rest,
        &ws,
        &settings,
        would_refuse,
        &mut client,
        &mut closing,
    ) {
        Ok(e) | Err(e) => e,
    };
    if let Some(c) = client.as_mut() {
        closing.flush(c, exit);
    }
    exit
}

/// A stop before any request, printed.
fn stopped(s: Stop) -> Exit {
    match s {
        Stop::Usage(line) => {
            out::stderr_line(&line);
            Exit::Usage
        }
        Stop::Shift(flag) => out::stop_need_value(&flag),
    }
}

/// One verb, from its words to its exit. `Err` ends it early.
fn verb(
    sub: &str,
    rest: &[String],
    ws: &dyn Fn(&str) -> String,
    settings: &Settings,
    would_refuse: bool,
    client: &mut Option<Client>,
    closing: &mut Closing,
) -> Result<Exit, Exit> {
    let connect = |client: &mut Option<Client>| -> Result<(), Exit> {
        match settings.client(would_refuse) {
            Ok(c) => {
                *client = Some(c);
                Ok(())
            }
            Err(e) => {
                out::stderr_line(&format!("Error: {e}"));
                Err(Exit::Error)
            }
        }
    };
    match sub {
        "new" => {
            let n = parse_new(rest).map_err(stopped)?;
            let brief = if n.brief_file.is_empty() {
                None
            } else if n.brief_file == "-" {
                let mut raw = Vec::new();
                // `cat > "$brief_src"`: a read error ends the copy there.
                let _ = std::io::stdin().lock().read_to_end(&mut raw);
                Some(Ok(raw))
            } else if Path::new(&n.brief_file).is_file() {
                Some(std::fs::read(&n.brief_file).map_err(|e| {
                    Raise::at(13, pyjson::os_error(&e, &n.brief_file).last_line()).text()
                }))
            } else {
                out::stderr_line(&format!("usage: brief file not found: {}", n.brief_file));
                return Err(Exit::Usage);
            };
            let body = match n.body(&ws(&n.workspace), brief) {
                Ok(b) => b,
                Err(text) => {
                    let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
                    return Err(Exit::Usage);
                }
            };
            connect(&mut *client)?;
            let c = client.as_mut().expect("connected");
            let split = call(c, true, "/cli/sidecar/new", &body)?;
            Ok(render(Reader::New, n.json, &split, c, closing))
        }
        "list" | "ls" => {
            let n = parse_simple("list", rest).map_err(stopped)?;
            connect(&mut *client)?;
            let c = client.as_mut().expect("connected");
            let q = format!("workspace={}", quote(&ws(&n.workspace)));
            let split = call(c, false, "/cli/sidecar/list", &q)?;
            Ok(render(Reader::List, n.json, &split, c, closing))
        }
        "stop" | "archive" => {
            let n = parse_simple(sub, rest).map_err(stopped)?;
            let (path, key, reader) = if sub == "stop" {
                ("/cli/sidecar/stop", "force", Reader::Stop)
            } else {
                ("/cli/sidecar/archive", "allStopped", Reader::Archive)
            };
            let body = confirm::dumps(&confirm::object(vec![
                ("workspace", s(&ws(&n.workspace))),
                ("name", s(&n.name)),
                (key, Value::Bool(n.switch)),
            ]));
            connect(&mut *client)?;
            let c = client.as_mut().expect("connected");
            let split = call(c, true, path, &body)?;
            Ok(render(reader, n.json, &split, c, closing))
        }
        "access" => {
            let (action, w) = parse_access(rest).map_err(stopped)?;
            let w = ws(&w);
            connect(&mut *client)?;
            let c = client.as_mut().expect("connected");
            let split = if action == "status" {
                call(
                    c,
                    false,
                    "/cli/sidecar/list",
                    &format!("workspace={}", quote(&w)),
                )?
            } else {
                let body = confirm::dumps(&confirm::object(vec![
                    ("workspace", s(&w)),
                    ("toggle", s("agents_manage")),
                    ("value", Value::Bool(action == "on")),
                ]));
                call(c, true, "/cli/agent-access/set", &body)?
            };
            Ok(render(Reader::Access, false, &split, c, closing))
        }
        _ => {
            let n = name_args(&verb_usage(sub), rest).map_err(stopped)?;
            let w = ws(&n.workspace);
            connect(&mut *client)?;
            let c = client.as_mut().expect("connected");
            let review_q = format!("workspace={}&name={}", quote(&w), quote(&n.name));
            match sub {
                "interrupt" => {
                    let body = confirm::dumps(&confirm::object(vec![
                        ("workspace", s(&w)),
                        ("name", s(&n.name)),
                    ]));
                    let split = call(c, true, "/cli/sidecar/interrupt", &body)?;
                    Ok(render(Reader::Interrupt, n.json, &split, c, closing))
                }
                "review" => {
                    let split = call(c, false, "/cli/sidecar/review", &review_q)?;
                    Ok(render(Reader::Review, n.json, &split, c, closing))
                }
                "land" => {
                    let split = call(c, false, "/cli/sidecar/review", &review_q)?;
                    let v = shown_review(&split, n.json, c, closing)?;
                    let (tip, onto, blocked) = land_words(&v);
                    if blocked == "already_landed" || blocked == "sidecar_retired" {
                        out::stderr_line("nothing to land");
                        return Err(Exit::Error);
                    }
                    ask(n.yes, "Land these on the base now?")?;
                    let mut b = vec![
                        ("workspace", s(&w)),
                        ("name", s(&n.name)),
                        ("expectTip", s(&tip)),
                        ("retire", Value::Bool(!n.keep)),
                    ];
                    if onto != "-" {
                        b.push(("expectOnto", s(&onto)));
                    }
                    let body = confirm::dumps(&confirm::object(b));
                    let split = call(c, true, "/cli/sidecar/land", &body)?;
                    Ok(render(Reader::Land, n.json, &split, c, closing))
                }
                _ => {
                    if n.discard {
                        let split = call(c, false, "/cli/sidecar/review", &review_q)?;
                        shown_review(&split, true, c, closing)?;
                        ask(
                            n.yes,
                            "Discard this work (commits deleted with the branch, files to the Trash)?",
                        )?;
                    }
                    let body = confirm::dumps(&confirm::object(vec![
                        ("workspace", s(&w)),
                        ("name", s(&n.name)),
                        ("discard", Value::Bool(n.discard)),
                        ("keepBranch", Value::Bool(n.keep_branch)),
                    ]));
                    let split = call(c, true, "/cli/sidecar/retire", &body)?;
                    Ok(render(Reader::Retire, n.json, &split, c, closing))
                }
            }
        }
    }
}

/// `_sidecar_split "$(_sidecar_send …)"`; `Err`: the transport's line
/// was printed and the command ends.
fn call(c: &mut Client, post: bool, path: &str, payload: &str) -> Result<Split, Exit> {
    confirm::sidecar_call(c, post, path, payload)
}

/// `_sidecar_fail`: its line, the refusal's closing line, its exit.
fn fail(split: &Split, c: &Client, closing: &mut Closing) -> Exit {
    out::stderr_line(&fail_line(&split.status, &split.body));
    closing.answer(&split.response(), c.passport().is_some());
    exit_for(&split.status)
}

/// `_sidecar_render <verb> <json> <raw>`.
fn render(reader: Reader, json: bool, split: &Split, c: &Client, closing: &mut Closing) -> Exit {
    if !split.ok() {
        return fail(split, c, closing);
    }
    if json {
        let mut text = split.body.clone();
        text.push(b'\n');
        return match out::write_flushed(&mut std::io::stdout().lock(), &text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    lay_out(reader, &split.body).print()
}

/// The review before `land` or `retire --discard`: a refusal ends the
/// command; else `_sidecar_print_review`, on stderr when `to_stderr`
/// (`>&2`). `Ok`: the answer, for the words after it.
fn shown_review(
    split: &Split,
    to_stderr: bool,
    c: &Client,
    closing: &mut Closing,
) -> Result<Value, Exit> {
    if !split.ok() {
        return Err(fail(split, c, closing));
    }
    let shown = lay_out(Reader::Review, &split.body);
    let shown = if to_stderr {
        // Python's stdout, sent to stderr, is flushed when it ends: after
        // a traceback, not before.
        Shown {
            stdout: String::new(),
            stderr: shown.stderr + &shown.stdout,
            exit: shown.exit,
        }
    } else {
        shown
    };
    let exit = shown.print();
    if exit != Exit::Ok {
        return Err(exit);
    }
    // The review read it as a dict, so this does too.
    Ok(load(&split.body).unwrap_or(Value::Null))
}

/// `read -r tip onto blocked < <(… print(v.get("tip") or "-", …))`:
/// the line's first two words, then the rest.
pub fn land_words(v: &Value) -> (String, String, String) {
    let dash = s("-");
    let word = |k: &str| py::str_of(ops::or(py_get(v, k, 4).ok().flatten(), &dash));
    let line = format!("{} {} {}", word("tip"), word("ontoHead"), word("blocked"));
    let line = line.split('\n').next().unwrap_or_default();
    read_three(line)
}

/// `read -r a b c` on one line, with the default `IFS`.
pub fn read_three(line: &str) -> (String, String, String) {
    let ws = |c: char| matches!(c, ' ' | '\t' | '\n');
    let mut rest = line.trim_start_matches(ws);
    let mut take = || {
        let end = rest.find(ws).unwrap_or(rest.len());
        let w = rest[..end].to_string();
        rest = rest[end..].trim_start_matches(ws);
        w
    };
    let a = take();
    let b = take();
    let c = rest.trim_end_matches(ws).to_string();
    (a, b, c)
}

/// `_sidecar_confirm "<question>"`. `Err`: the command ends.
fn ask(yes: bool, question: &str) -> Result<(), Exit> {
    if yes {
        return Ok(());
    }
    if !(prompt::stdin_is_tty() && prompt::stderr_is_tty()) {
        out::stderr_line(NO_TERMINAL);
        return Err(Exit::Usage);
    }
    let answer = prompt::ask(&format!("{question} [y/N] "));
    let text = String::from_utf8_lossy(answer.expose()).into_owned();
    if matches!(
        text.trim_matches(|c| matches!(c, ' ' | '\t' | '\n')),
        "y" | "Y" | "yes" | "YES"
    ) {
        return Ok(());
    }
    out::stderr_line("not done");
    Err(Exit::Error)
}

/// `json.load(sys.stdin)` at line 3 of each reader.
fn load(body: &[u8]) -> Result<Value, Raise> {
    let raw = String::from_utf8_lossy(body);
    pyjson::loads(&raw).map_err(|f| Raise::at(3, f.last_line()))
}

/// The shell's Python on a 200's body.
pub fn lay_out(reader: Reader, body: &[u8]) -> Shown {
    let mut o = String::new();
    let mut e = String::new();
    let r = load(body).and_then(|v| match reader {
        Reader::New => new_lines(&v, &mut o, &mut e),
        Reader::Stop => stop_lines(&v, &mut o),
        Reader::Archive => archive_lines(&v, &mut o),
        Reader::List => list_lines(&v, &mut o),
        Reader::Access => access_lines(&v, &mut o),
        Reader::Interrupt => interrupt_lines(&v, &mut o),
        Reader::Review => review_lines(&v, &mut o),
        Reader::Land => land_lines(&v, &mut o),
        Reader::Retire => retire_lines(&v, &mut o),
    });
    match r {
        Ok(()) => Shown {
            stdout: o,
            stderr: e,
            exit: Exit::Ok,
        },
        Err(r) => raised(o, e, &r),
    }
}

fn push_line(o: &mut String, text: &str) {
    o.push_str(text);
    o.push('\n');
}

/// `x[:n]`, or the exception at `line`.
pub fn py_slice(v: &Value, n: usize, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Str(t) => Ok(Value::Str(match t.as_str() {
            Some(text) => text.chars().take(n).collect::<String>().into(),
            None => {
                // A lone surrogate: the code points, as JSON escapes.
                let escaped: String = t
                    .code_points()
                    .into_iter()
                    .take(n)
                    .map(|cp| format!("\\u{cp:04x}"))
                    .collect();
                match crate::out::json::parse(&format!("\"{escaped}\"")) {
                    Ok(Value::Str(cut)) => cut,
                    _ => t.clone(),
                }
            }
        })),
        Value::Array(items) => Ok(Value::Array(items.iter().take(n).cloned().collect())),
        Value::Object(_) => Err(Raise::at(line, "TypeError: unhashable type: 'slice'")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `(x or default)[:n]` as `%s` prints it.
fn cut(x: Option<&Value>, default: &str, n: usize, line: u32) -> Result<String, Raise> {
    let d = s(default);
    Ok(py::str_of(&py_slice(ops::or(x, &d), n, line)?))
}

/// `"" if n == 1 else "s"`.
fn plural(n: &Value) -> &'static str {
    if ops::py_eq(n, &Value::Int("1".into())) {
        ""
    } else {
        "s"
    }
}

/// `d.get(key, default)`, `d` a dict for sure.
fn at_or(v: &Value, key: &str, default: Value, line: u32) -> Result<Value, Raise> {
    Ok(py_get(v, key, line)?.cloned().unwrap_or(default))
}

/// `str(d.get(key))`, `d` a dict for sure.
fn str_at(v: &Value, key: &str, line: u32) -> Result<String, Raise> {
    Ok(ops::s_of(py_get(v, key, line)?))
}

fn truthy_at(v: &Value, key: &str, line: u32) -> Result<bool, Raise> {
    Ok(py_get(v, key, line)?.is_some_and(py::truthy))
}

fn zero() -> Value {
    Value::Int("0".into())
}

fn new_lines(v: &Value, o: &mut String, e: &mut String) -> Result<(), Raise> {
    push_line(o, &py::str_of(ops::index(v, "address", 4)?));
    let what = if truthy_at(v, "resumed", 5)? {
        "resumed"
    } else {
        "opened"
    };
    let empty = Value::Object(Vec::new());
    let brief = ops::or(py_get(v, "brief", 6)?, &empty).clone();
    let mut extra = String::new();
    if py::truthy(&brief) {
        let path = str_at(&brief, "path", 7)?;
        let bytes = at_or(&brief, "bytes", zero(), 7)?;
        extra = format!(" with brief {path} ({} bytes)", py_d(&bytes, 7)?);
    }
    let wt = ops::or(py_get(v, "worktree", 8)?, &empty).clone();
    if py::truthy(&wt) {
        let dir = str_at(&wt, "dir", 10)?;
        let branch = str_at(&wt, "branch", 10)?;
        extra.push_str(&format!(" in {dir} on branch {branch}"));
        if truthy_at(&wt, "base", 11)? {
            let base = str_at(&wt, "base", 12)?;
            let sha = cut(py_get(&wt, "baseSha", 12)?, "", 10, 12)?;
            extra.push_str(&format!(" (from {base} {sha})"));
        }
    }
    let address = py::str_of(ops::index(v, "address", 13)?);
    let harness = str_at(v, "harness", 13)?;
    let again = py::str_of(ops::index(v, "address", 13)?);
    e.push_str(&format!(
        "{what} {address} ({harness}){extra}. Message it: {CLI} msg {again} \"...\"\n"
    ));
    Ok(())
}

fn stop_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let line = if truthy_at(v, "stopped", 4)? {
        format!("stopped {}", py::str_of(ops::index(v, "address", 4)?))
    } else {
        format!(
            "{} was already asleep",
            py::str_of(ops::index(v, "address", 4)?)
        )
    };
    push_line(o, &line);
    Ok(())
}

fn archive_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    if truthy_at(v, "address", 4)? {
        let address = ops::index(v, "address", 5)?;
        let shown = py::str_of(address);
        let last = match address {
            Value::Str(_) => shown.rsplit('/').next().unwrap_or_default().to_string(),
            other => return Err(Raise::no_attribute(5, other, "split")),
        };
        push_line(
            o,
            &format!(
                "archived {shown}. Its conversation stays: {CLI} sidecar new {last} --harness <the same harness> resumes it."
            ),
        );
        return Ok(());
    }
    let n = at_or(v, "count", zero(), 7)?;
    let ending = plural(&n);
    let names = if py::truthy(&n) {
        let none = Value::Array(Vec::new());
        format!(
            ": {}",
            ops::join(ops::or(py_get(v, "archived", 8)?, &none), ", ", 8)?
        )
    } else {
        String::new()
    };
    push_line(
        o,
        &format!("archived {} stopped sidecar{ending}{names}", py_d(&n, 8)?),
    );
    let none = Value::Array(Vec::new());
    for sk in py_iter(ops::or(py_get(v, "skipped", 9)?, &none), 9)? {
        let address = str_at(&sk, "address", 10)?;
        let hint = str_at(&sk, "hint", 10)?;
        push_line(o, &format!("not archived {address}: {hint}"));
    }
    Ok(())
}

fn list_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let rows = at_or(v, "sidecars", Value::Array(Vec::new()), 4)?;
    let workspace = str_at(v, "workspace", 5)?;
    let live = at_or(v, "live", zero(), 5)?;
    let count = Value::Int(py_len(&rows, 5)?.to_string());
    let cap = at_or(v, "cap", zero(), 5)?;
    let on = if truthy_at(v, "agentsCanManage", 5)? {
        "on"
    } else {
        "off"
    };
    push_line(
        o,
        &format!(
            "{workspace}: {} live of {} (cap {}); its agent may open sidecars: {on}",
            py_d(&live, 5)?,
            py_d(&count, 5)?,
            py_d(&cap, 5)?
        ),
    );
    if !py::truthy(&rows) {
        push_line(o, "(no sidecars)");
        return Ok(());
    }
    let row = |cells: [&Value; 7]| {
        format!(
            "{} {} {} {} {} {} {}",
            py_left(cells[0], 28),
            py_left(cells[1], 20),
            py_left(cells[2], 8),
            py_left(cells[3], 7),
            py_left(cells[4], 14),
            py_left(cells[5], 16),
            py::str_of(cells[6])
        )
    };
    push_line(
        o,
        &row([
            &s("ADDRESS"),
            &s("NAME"),
            &s("HARNESS"),
            &s("STATE"),
            &s("BY"),
            &s("SINCE"),
            &s("TASK"),
        ]),
    );
    let dash = s("-");
    let empty = Value::Object(Vec::new());
    for r in py_iter(&rows, 10)? {
        let since = ops::or(py_get(&r, "lastSeenAt", 11)?, &zero()).clone();
        let when = if py::truthy(&since) {
            let secs = time::seconds(&since, 0).map_err(|e| Raise::at(12, e))?;
            time::strftime_local(secs, "%Y-%m-%d %H:%M").map_err(|e| Raise::at(12, e))?
        } else {
            "-".to_string()
        };
        let wt = ops::or(py_get(&r, "worktree", 13)?, &empty).clone();
        let mut task = "-".to_string();
        if py::truthy(&wt) {
            let state = py::str_of(ops::or(py_get(&r, "taskState", 16)?, &dash));
            let branch = py::str_of(ops::or(py_get(&wt, "branch", 16)?, &s("")));
            task = format!("{state} {branch}");
            if truthy_at(&wt, "ahead", 17)? {
                let ahead = py_get(&wt, "ahead", 18)?.cloned().unwrap_or(Value::Null);
                task.push_str(&format!(" +{}", py_d(&ahead, 18)?));
            }
            if truthy_at(&wt, "dirty", 19)? {
                task.push_str(" (uncommitted)");
            }
        }
        let address = ops::or(py_get(&r, "address", 21)?, &dash).clone();
        let name = py_slice(ops::or(py_get(&r, "name", 21)?, &dash), 20, 21)?;
        let harness = ops::or(py_get(&r, "harness", 21)?, &dash).clone();
        let state = ops::or(py_get(&r, "state", 22)?, &dash).clone();
        let by = py_slice(ops::or(py_get(&r, "createdBy", 22)?, &s("app")), 14, 22)?;
        push_line(
            o,
            &row([&address, &name, &harness, &state, &by, &s(&when), &s(&task)]),
        );
    }
    let w = ops::or(py_get(v, "worktrees", 23)?, &empty).clone();
    if truthy_at(&w, "open", 24)? {
        let open = at_or(&w, "open", zero(), 25)?;
        let cap = at_or(&w, "cap", zero(), 25)?;
        push_line(
            o,
            &format!(
                "worktree sidecars: {} of {} (retire one to free a slot)",
                py_d(&open, 25)?,
                py_d(&cap, 25)?
            ),
        );
    }
    Ok(())
}

fn access_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let on = if truthy_at(v, "agentsCanManage", 4)? {
        "on"
    } else {
        "off"
    };
    let workspace = str_at(v, "workspace", 4)?;
    push_line(
        o,
        &format!("Let this agent manage its sidecars: {on} for {workspace}"),
    );
    Ok(())
}

fn interrupt_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    if truthy_at(v, "interrupted", 4)? {
        let address = py::str_of(ops::index(v, "address", 5)?);
        let key = str_at(v, "key", 5)?;
        push_line(
            o,
            &format!("interrupted {address} (sent {key}); it keeps running"),
        );
    } else {
        let address = py::str_of(ops::index(v, "address", 7)?);
        push_line(o, &format!("{address} is asleep; nothing to interrupt"));
    }
    Ok(())
}

fn review_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let address = py::str_of(ops::index(v, "address", 4)?);
    let branch = str_at(v, "branch", 4)?;
    let dir = str_at(v, "dir", 4)?;
    let state = str_at(v, "taskState", 4)?;
    push_line(o, &format!("{address}  {branch} in {dir}  ({state})"));
    if truthy_at(v, "retiredAt", 5)? {
        let how = str_at(v, "retiredHow", 6)?;
        push_line(o, &format!("retired ({how}); nothing to land"));
        return Ok(());
    }
    let none = Value::Array(Vec::new());
    let commits = ops::or(py_get(v, "commits", 7)?, &none).clone();
    let count = py_len(&commits, 8)?;
    let onto = str_at(v, "onto", 8)?;
    let head = cut(py_get(v, "ontoHead", 8)?, "-", 10, 8)?;
    push_line(
        o,
        &format!(
            "{count} commit{} to land on {onto} (at {head}):",
            if count == 1 { "" } else { "s" }
        ),
    );
    for c in py_iter(&commits, 9)? {
        let sha = py::str_of(&py_slice(ops::index(&c, "sha", 10)?, 10, 10)?);
        let subject = py::str_of(&at_or(&c, "subject", s(""), 10)?);
        let merge = if truthy_at(&c, "merge", 10)? {
            "  (merge)"
        } else {
            ""
        };
        push_line(o, &format!("  {sha} {subject}{merge}"));
    }
    let files = ops::or(py_get(v, "files", 11)?, &none).clone();
    if py::truthy(&files) {
        push_line(o, "files:");
        for f in py_iter(&files, 14)? {
            let status = str_at(&f, "status", 15)?;
            let path = str_at(&f, "path", 15)?;
            push_line(o, &format!("  {status} {path}"));
        }
    }
    let dirty = ops::or(py_get(v, "dirty", 16)?, &none).clone();
    if py::truthy(&dirty) {
        push_line(
            o,
            &format!(
                "uncommitted in the worktree (never landed): {}",
                ops::join(&dirty, ", ", 18)?
            ),
        );
    }
    let main_branch = str_at(v, "mainBranch", 20)?;
    let onto20 = str_at(v, "onto", 20)?;
    let main_dirty = ops::join(ops::or(py_get(v, "mainDirty", 21)?, &none), ", ", 21)?;
    let onto24 = str_at(v, "onto", 24)?;
    let why = |b: &str| -> Option<String> {
        Some(match b {
            "wrong_branch" => format!("the main checkout is on {main_branch}, not {onto20}"),
            "main_has_uncommitted_changes" => {
                format!("the main checkout has uncommitted changes: {main_dirty}")
            }
            "operation_in_progress" => {
                "a merge, rebase or cherry-pick is in progress in the main checkout".into()
            }
            "has_merge" => "a merge commit is in range; rebase the branch first".into(),
            "already_landed" => format!("every commit is already on {onto24}"),
            "stale" => "the base or the branch is gone".into(),
            _ => return None,
        })
    };
    let b = py_get(v, "blocked", 27)?.cloned().unwrap_or(Value::Null);
    let line = if !py::truthy(&b) {
        format!(
            "land: ready (tip {})",
            cut(py_get(v, "tip", 28)?, "", 10, 28)?
        )
    } else {
        let reason = match &b {
            Value::Str(_) => why(&py::str_of(&b)).unwrap_or_else(|| py::str_of(&b)),
            Value::Array(_) | Value::Object(_) => {
                return Err(Raise::at(
                    28,
                    format!("TypeError: unhashable type: '{}'", py::type_name(&b)),
                ))
            }
            other => py::str_of(other),
        };
        format!("land: not now ({}): {reason}", py::str_of(&b))
    };
    push_line(o, &line);
    Ok(())
}

fn land_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let n = at_or(v, "count", zero(), 4)?;
    let ending = plural(&n);
    let address = py::str_of(ops::index(v, "address", 5)?);
    let onto = str_at(v, "onto", 5)?;
    let from = cut(py_get(v, "from", 5)?, "", 10, 5)?;
    let to = cut(py_get(v, "to", 5)?, "", 10, 5)?;
    push_line(
        o,
        &format!(
            "landed {} commit{ending} from {address} on {onto} ({from}..{to})",
            py_d(&n, 5)?
        ),
    );
    let r = py_get(v, "retired", 6)?.cloned();
    let e = py_get(v, "retireError", 7)?.cloned();
    if let Some(r @ Value::Object(_)) = &r {
        let address = py::str_of(ops::index(v, "address", 9)?);
        let deleted = if truthy_at(r, "branchDeleted", 9)? {
            ", branch deleted"
        } else {
            ""
        };
        push_line(
            o,
            &format!("retired {address}: worktree in the Trash{deleted}"),
        );
    } else if let Some(e @ Value::Object(_)) = &e {
        let code = str_at(e, "code", 11)?;
        let hint = str_at(e, "hint", 11)?;
        push_line(o, &format!("not retired: {code}: {hint}"));
        let address = py::str_of(ops::index(v, "address", 12)?);
        push_line(
            o,
            &format!("retire it later: {CLI} sidecar retire {address}"),
        );
    }
    Ok(())
}

fn retire_lines(v: &Value, o: &mut String) -> Result<(), Raise> {
    let address = py::str_of(ops::index(v, "address", 4)?);
    let how = str_at(v, "how", 4)?;
    let deleted = if truthy_at(v, "branchDeleted", 4)? {
        ", branch deleted"
    } else {
        ", branch kept"
    };
    push_line(
        o,
        &format!("retired {address} ({how}): worktree in the Trash{deleted}"),
    );
    let none = Value::Array(Vec::new());
    let dc = ops::or(py_get(v, "discardedCommits", 5)?, &none).clone();
    if py::truthy(&dc) {
        let n = py_len(&dc, 7)?;
        let last = py_last(&dc, 7)?;
        let sha = py::str_of(&py_slice(ops::index(&last, "sha", 7)?, 10, 7)?);
        push_line(
            o,
            &format!(
                "discarded {n} commit(s); until git cleans up, git branch <name> {sha} brings them back"
            ),
        );
    }
    Ok(())
}

/// `x[-1]` on a truthy value, or the exception at `line`.
fn py_last(v: &Value, line: u32) -> Result<Value, Raise> {
    match v {
        Value::Array(items) => Ok(items.last().cloned().unwrap_or(Value::Null)),
        Value::Str(t) => Ok(t
            .code_points()
            .last()
            .map(|cp| Value::Str(super::code_point(*cp)))
            .unwrap_or(Value::Null)),
        Value::Object(_) => Err(Raise::at(line, "KeyError: -1")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

#[cfg(test)]
mod tests;
