//! `k5 changes …`: the Changes tab from the command line, as the shell
//! CLI's `cmd_changes` answers it (`cli/src/changes.sh`,
//! `changes-open.sh`, `changes-notes.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings, a re-minted passport taken at once, and the two gates
//!    ([`crate::prelude`]). `help` second, or `-h` or `--help` anywhere,
//!    skip the connection gate; the virtual desktop's guard stops all but
//!    a line with `-h` or `--help` in it.
//! 2. `changes` is an `id` tool: with `HOOK_SOCK` set, the stale-passport
//!    or require-passport lines, exit 3.
//! 3. `-h` or `--help` anywhere: the help page (`notes`'s own after
//!    `notes`). `help`: the help page. No verb, or a flag first: the list.
//!    An unknown verb: its line and the help page on stderr, exit 2.
//! 4. The verb's words ([`parse`]); a bad one is `_changes_die`: `Error:
//!    <hint>` (not with `--json`) and the JSON error object on stderr,
//!    exit 2.
//! 5. `_changes_target`: with `--workspace`, a folder as `cd` names it
//!    (and its physical path for file words), else the daemon's
//!    `/cli/workspace/resolve`, else the word; with none, the git checkout
//!    under the workspace you stand in, else the workspace (left out in a
//!    session with a passport, where the daemon takes the passport's).
//! 6. The requests (`cli_send_json`: the cell socket first with a
//!    passport and a live socket, then TCP; a POST again over TCP only
//!    after a refused connect or "not served"), each status other than 200
//!    the error reader ([`refusal`]): exit 1 for no answer, else 3.
//! 7. What the verb prints ([`render`], [`diff`]), then the EXIT trap's
//!    lines.
//!
//! Hand-backs to the shell, decided before anything is printed or sent:
//! a word that is not UTF-8, `HOOK_SOCK` with a `CODEX_` variable, a port
//! curl won't take, a `python3` the binary can't tell, and a
//! `--workspace` folder `cd` reads as an option or one `CDPATH` would
//! move (bash prints its own error there). An answer that is not
//! UTF-8 is read with U+FFFD for each bad byte.

pub mod diff;
pub mod render;

use super::sim::{cd_pwd, cd_searches, die, logical_pwd, Die};
use super::{curl_port, get, py_space, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["changes"];
/// The binary answers it; [`run`] hands the edge cases back.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "changes";

fn usage(hint: impl Into<String>) -> Die {
    die("usage", hint)
}

fn name() -> &'static str {
    cli_name!()
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

fn dumps(v: &Value) -> String {
    json::to_string(v, &Options::default()).expect("a value from JSON or words always prints")
}

/// `notes resolve`, `reopen` or `delete`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdsVerb {
    Resolve,
    Reopen,
    Delete,
}

impl IdsVerb {
    fn word(self) -> &'static str {
        match self {
            IdsVerb::Resolve => "resolve",
            IdsVerb::Reopen => "reopen",
            IdsVerb::Delete => "delete",
        }
    }
}

/// A command line, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    /// The help page (`notes`: the notes page) on stdout.
    Help {
        notes: bool,
    },
    /// An unknown verb (`notes`: of `notes`): its line, the page on stderr.
    Unknown {
        notes: bool,
        word: String,
    },
    List {
        ws: String,
        base: String,
    },
    Diff(diff::Opts),
    Stage {
        unstage: bool,
        ws: String,
        all: bool,
        paths: Vec<String>,
    },
    Discard {
        ws: String,
        yes: bool,
        staged: bool,
        paths: Vec<String>,
    },
    Commit {
        ws: String,
        all: bool,
        msg: String,
    },
    NotesList {
        ws: String,
        status: String,
        file: String,
    },
    NotesIds {
        verb: IdsVerb,
        ws: String,
        ids: Vec<String>,
    },
    NotesAdd {
        ws: String,
        file: String,
        side: &'static str,
        start: String,
        end: String,
        msg: String,
    },
    NotesEdit {
        ws: String,
        id: String,
        msg: String,
    },
    NotesSend {
        ws: String,
        wake: bool,
        ids: Vec<String>,
    },
    Open {
        ws: String,
    },
    Init {
        ws: String,
    },
}

impl Verb {
    /// The `--workspace` word, if the verb has one.
    pub fn ws(&self) -> Option<&str> {
        match self {
            Verb::Help { .. } | Verb::Unknown { .. } => None,
            Verb::Diff(o) => Some(&o.ws),
            Verb::List { ws, .. }
            | Verb::Stage { ws, .. }
            | Verb::Discard { ws, .. }
            | Verb::Commit { ws, .. }
            | Verb::NotesList { ws, .. }
            | Verb::NotesIds { ws, .. }
            | Verb::NotesAdd { ws, .. }
            | Verb::NotesEdit { ws, .. }
            | Verb::NotesSend { ws, .. }
            | Verb::Open { ws }
            | Verb::Init { ws } => Some(ws),
        }
    }
}

/// The words a verb's loop reads, with `_CH_JSON` as it stands.
pub struct Words<'a> {
    w: &'a [&'a str],
    i: usize,
    pub json: bool,
}

impl<'a> Words<'a> {
    fn new(w: &'a [&'a str]) -> Self {
        Words {
            w,
            i: 0,
            json: false,
        }
    }

    fn word(&mut self) -> Option<&'a str> {
        let w = self.w.get(self.i).copied();
        self.i += 1;
        w
    }

    /// A value flag's word: `Err` is `_changes_die usage <hint> 2`.
    fn value(&mut self, hint: &str) -> Result<String, Die> {
        match self.w.get(self.i) {
            Some(v) => {
                self.i += 1;
                Ok((*v).to_string())
            }
            None => Err(self.die(usage(hint))),
        }
    }

    /// The words left (after `--`).
    fn rest(&mut self) -> Vec<String> {
        let r = self.w[self.i.min(self.w.len())..]
            .iter()
            .map(|w| w.to_string())
            .collect();
        self.i = self.w.len();
        r
    }

    fn die(&self, d: Die) -> Die {
        Die {
            json: self.json,
            ..d
        }
    }

    /// `--json`, `--workspace`, `-w`, `--workspace=…`: true when `w` was
    /// one of them.
    fn common(&mut self, w: &str, ws: &mut String) -> Result<bool, Die> {
        match w {
            "--json" => self.json = true,
            "--workspace" | "-w" => *ws = self.value("--workspace needs a path")?,
            _ => match w.strip_prefix("--workspace=") {
                Some(v) => *ws = v.to_string(),
                None => return Ok(false),
            },
        }
        Ok(true)
    }
}

/// `cmd_changes`'s words after `changes`: the verb, and `_CH_JSON` at its
/// end; or the refusal, with `_CH_JSON` as it stood.
pub fn parse(words: &[&str]) -> Result<(Verb, bool), Die> {
    let sub = words.first().copied().unwrap_or("");
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return Ok((
            Verb::Help {
                notes: matches!(sub, "notes" | "note"),
            },
            false,
        ));
    }
    let n = name();
    let rest = if sub.is_empty() || sub.starts_with('-') {
        words
    } else {
        &words[1..]
    };
    let mut w = Words::new(rest);
    let verb = match sub {
        "" | "list" | "ls" | "status" => list(&mut w)?,
        _ if sub.starts_with('-') => list(&mut w)?,
        "diff" => Verb::Diff(diff::parse(&mut w)?),
        "stage" | "add" => stage(&mut w, false)?,
        "unstage" => stage(&mut w, true)?,
        "discard" => discard(&mut w)?,
        "commit" => commit(&mut w)?,
        "notes" | "note" => notes(&mut w)?,
        "open" | "init" => {
            let mut ws = String::new();
            while let Some(a) = w.word() {
                if !w.common(a, &mut ws)? {
                    return Err(w.die(usage(format!(
                        "unknown argument for {n} changes {sub}: {a}"
                    ))));
                }
            }
            if sub == "open" {
                Verb::Open { ws }
            } else {
                Verb::Init { ws }
            }
        }
        "help" => Verb::Help { notes: false },
        other => Verb::Unknown {
            notes: false,
            word: other.to_string(),
        },
    };
    Ok((verb, w.json))
}

fn list(w: &mut Words) -> Result<Verb, Die> {
    let n = name();
    let (mut ws, mut base) = (String::new(), String::new());
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        if a == "--base" {
            base = w.value("--base needs a ref")?;
        } else if let Some(v) = a.strip_prefix("--base=") {
            base = v.to_string();
        } else {
            return Err(w.die(usage(format!(
                "unknown argument for {n} changes: {a} (see {n} changes --help)"
            ))));
        }
    }
    Ok(Verb::List { ws, base })
}

fn stage(w: &mut Words, unstage: bool) -> Result<Verb, Die> {
    let n = name();
    let verb = if unstage { "unstage" } else { "stage" };
    let (mut ws, mut all, mut paths) = (String::new(), false, Vec::new());
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        match a {
            "--all" | "-A" => {
                if unstage {
                    return Err(w.die(usage(format!("{n} changes unstage takes paths (no --all)"))));
                }
                all = true;
            }
            "--" => paths.extend(w.rest()),
            _ if a.starts_with('-') => {
                return Err(w.die(usage(format!("unknown flag for {n} changes {verb}: {a}"))))
            }
            _ => paths.push(a.to_string()),
        }
    }
    if all && !paths.is_empty() {
        return Err(w.die(usage(format!(
            "{n} changes stage: give paths or --all, not both"
        ))));
    }
    if !all && paths.is_empty() {
        let tail = if unstage { "" } else { " | --all" };
        return Err(w.die(usage(format!("usage: {n} changes {verb} <path>...{tail}"))));
    }
    Ok(Verb::Stage {
        unstage,
        ws,
        all,
        paths,
    })
}

fn discard(w: &mut Words) -> Result<Verb, Die> {
    let n = name();
    let (mut ws, mut yes, mut staged, mut paths) = (String::new(), false, false, Vec::new());
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        match a {
            "--yes" | "-y" => yes = true,
            "--staged" | "--include-staged" => staged = true,
            "--" => paths.extend(w.rest()),
            _ if a.starts_with('-') => {
                return Err(w.die(usage(format!("unknown flag for {n} changes discard: {a}"))))
            }
            _ => paths.push(a.to_string()),
        }
    }
    if paths.is_empty() {
        return Err(w.die(usage(format!(
            "usage: {n} changes discard <path>... [--staged] [--yes]"
        ))));
    }
    Ok(Verb::Discard {
        ws,
        yes,
        staged,
        paths,
    })
}

/// `-m <it>` again: the messages joined by a blank line.
fn add_msg(msg: &mut String, have: &mut bool, v: String) {
    if *have {
        msg.push_str("\n\n");
        msg.push_str(&v);
    } else {
        *msg = v;
    }
    *have = true;
}

fn commit(w: &mut Words) -> Result<Verb, Die> {
    let n = name();
    let (mut ws, mut all, mut msg, mut have) = (String::new(), false, String::new(), false);
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        match a {
            "-m" | "--message" => {
                let v = w.value("-m needs a message")?;
                add_msg(&mut msg, &mut have, v);
            }
            "--all" | "-a" => all = true,
            _ => match a.strip_prefix("--message=") {
                Some(v) => {
                    msg = v.to_string();
                    have = true;
                }
                None => {
                    return Err(w.die(usage(format!(
                        "unknown argument for {n} changes commit: {a} (usage: {n} changes commit -m \"message\" [--all])"
                    ))))
                }
            },
        }
    }
    // `tr -d '[:space:]'`: ASCII spaces only.
    if !have || msg.bytes().all(|b| b" \t\n\x0b\x0c\r".contains(&b)) {
        return Err(w.die(usage(format!(
            "usage: {n} changes commit -m \"message\" [--all]"
        ))));
    }
    Ok(Verb::Commit { ws, all, msg })
}

fn notes(w: &mut Words) -> Result<Verb, Die> {
    let n = name();
    let sub = w.w.first().copied().unwrap_or("");
    if sub == "help" {
        return Ok(Verb::Help { notes: true });
    }
    if !(sub.is_empty() || sub.starts_with('-')) {
        w.i = 1;
    }
    match sub {
        _ if sub.is_empty() || sub.starts_with('-') => notes_list(w),
        "list" | "ls" => notes_list(w),
        "resolve" | "done" => notes_ids(w, IdsVerb::Resolve),
        "reopen" => notes_ids(w, IdsVerb::Reopen),
        "delete" | "rm" => notes_ids(w, IdsVerb::Delete),
        "add" => {
            let (mut ws, mut file, mut lines, mut side) =
                (String::new(), String::new(), String::new(), "new");
            let (mut msg, mut have) = (String::new(), false);
            while let Some(a) = w.word() {
                if w.common(a, &mut ws)? {
                    continue;
                }
                match a {
                    "--line" | "--lines" | "-L" => {
                        lines = w.value("--line needs <n> or <n>-<m>")?
                    }
                    "--old" => side = "old",
                    "-m" | "--message" => {
                        let v = w.value("-m needs the comment")?;
                        add_msg(&mut msg, &mut have, v);
                    }
                    _ if a.starts_with("--line=") || a.starts_with("--lines=") => {
                        lines = a.split_once('=').map_or("", |(_, v)| v).to_string();
                    }
                    _ if a.starts_with('-') => {
                        return Err(w.die(usage(format!(
                            "unknown flag for {n} changes notes add: {a}"
                        ))))
                    }
                    _ => {
                        if !file.is_empty() {
                            return Err(
                                w.die(usage(format!("{n} changes notes add takes one file")))
                            );
                        }
                        file = a.to_string();
                    }
                }
            }
            if file.is_empty() || lines.is_empty() || !have {
                return Err(w.die(usage(format!(
                    "usage: {n} changes notes add <path> --line <n>[-<m>] [--old] -m \"comment\""
                ))));
            }
            let (start, end) = match lines.split_once('-') {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (lines.clone(), lines.clone()),
            };
            let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
            if !(digits(&start) && digits(&end)) {
                return Err(w.die(usage("--line takes <n> or <n>-<m> (numbers)")));
            }
            Ok(Verb::NotesAdd {
                ws,
                file,
                side,
                start,
                end,
                msg,
            })
        }
        "edit" | "update" => {
            let (mut ws, mut id, mut msg, mut have) =
                (String::new(), String::new(), String::new(), false);
            while let Some(a) = w.word() {
                if w.common(a, &mut ws)? {
                    continue;
                }
                match a {
                    "-m" | "--message" => {
                        let v = w.value("-m needs the comment")?;
                        add_msg(&mut msg, &mut have, v);
                    }
                    _ if a.starts_with('-') => {
                        return Err(w.die(usage(format!(
                            "unknown flag for {n} changes notes edit: {a}"
                        ))))
                    }
                    _ => {
                        if !id.is_empty() {
                            return Err(
                                w.die(usage(format!("{n} changes notes edit takes one note id")))
                            );
                        }
                        id = a.to_string();
                    }
                }
            }
            if id.is_empty() || !have {
                return Err(w.die(usage(format!(
                    "usage: {n} changes notes edit <id> -m \"comment\""
                ))));
            }
            Ok(Verb::NotesEdit { ws, id, msg })
        }
        "send" => {
            let (mut ws, mut wake, mut ids) = (String::new(), true, Vec::new());
            while let Some(a) = w.word() {
                if w.common(a, &mut ws)? {
                    continue;
                }
                match a {
                    "--no-wake" => wake = false,
                    _ if a.starts_with('-') => {
                        return Err(w.die(usage(format!(
                            "unknown flag for {n} changes notes send: {a}"
                        ))))
                    }
                    _ => ids.push(a.to_string()),
                }
            }
            Ok(Verb::NotesSend { ws, wake, ids })
        }
        other => Ok(Verb::Unknown {
            notes: true,
            word: other.to_string(),
        }),
    }
}

fn notes_list(w: &mut Words) -> Result<Verb, Die> {
    let n = name();
    let (mut ws, mut status, mut file) = (String::new(), String::new(), String::new());
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        match a {
            "--status" => status = w.value("--status needs open|unsent|sent|resolved|all")?,
            "--all" => status = "all".into(),
            "--file" => file = w.value("--file needs a path")?,
            _ => {
                if let Some(v) = a.strip_prefix("--status=") {
                    status = v.to_string();
                } else if let Some(v) = a.strip_prefix("--file=") {
                    file = v.to_string();
                } else {
                    return Err(w.die(usage(format!(
                        "unknown argument for {n} changes notes: {a} (see {n} changes notes --help)"
                    ))));
                }
            }
        }
    }
    Ok(Verb::NotesList { ws, status, file })
}

fn notes_ids(w: &mut Words, verb: IdsVerb) -> Result<Verb, Die> {
    let n = name();
    let (mut ws, mut ids) = (String::new(), Vec::new());
    while let Some(a) = w.word() {
        if w.common(a, &mut ws)? {
            continue;
        }
        if a.starts_with('-') {
            return Err(w.die(usage(format!(
                "unknown flag for {n} changes notes {}: {a}",
                verb.word()
            ))));
        }
        ids.push(a.to_string());
    }
    if ids.is_empty() {
        return Err(w.die(usage(format!(
            "usage: {n} changes notes {} <note id>...",
            verb.word()
        ))));
    }
    Ok(Verb::NotesIds { verb, ws, ids })
}

// ---------------------------------------------------------------------
// Bodies and queries.

/// `_changes_q`: `path=<it>&`, or nothing.
pub fn q(path: &str) -> String {
    if path.is_empty() {
        String::new()
    } else {
        format!("path={}&", quote(path))
    }
}

/// `${q%&}`.
fn trim_amp(q: &str) -> &str {
    q.strip_suffix('&').unwrap_or(q)
}

/// `_changes_body KIND …`.
pub enum Body<'a> {
    All,
    File(&'a str),
    Commit(&'a str),
    Discard { staged: bool, files: &'a [String] },
}

/// `_changes_body`: `path` (when set), then the kind's fields.
pub fn body(path: &str, kind: Body) -> String {
    let mut d: Vec<(Text, Value)> = Vec::new();
    if !path.is_empty() {
        d.push((Text::from("path"), s(path)));
    }
    match kind {
        Body::All => {}
        Body::File(f) => d.push((Text::from("filePath"), s(f))),
        Body::Commit(m) => d.push((Text::from("message"), s(m))),
        Body::Discard { staged, files } => {
            d.push((Text::from("includeStaged"), Value::Bool(staged)));
            d.push((
                Text::from("files"),
                Value::Array(files.iter().map(|f| s(f)).collect()),
            ));
        }
    }
    dumps(&Value::Object(d))
}

/// `_changes_json KEY=VALUE…`: `path` (when set); `k[]` collects a list,
/// `k#` is a number, `k?` a bool (`1`), else a string.
pub fn json_body(path: &str, pairs: &[String]) -> String {
    let mut d: Vec<(Text, Value)> = Vec::new();
    if !path.is_empty() {
        d.push((Text::from("path"), s(path)));
    }
    let set = |d: &mut Vec<(Text, Value)>, k: &str, v: Value| {
        let k = Text::from(k);
        match d.iter_mut().find(|(x, _)| *x == k) {
            Some((_, slot)) => *slot = v,
            None => d.push((k, v)),
        }
    };
    for p in pairs {
        let (k, v) = p.split_once('=').unwrap_or((p.as_str(), ""));
        if let Some(k) = k.strip_suffix("[]") {
            let key = Text::from(k);
            match d.iter_mut().find(|(x, _)| *x == key) {
                Some((_, Value::Array(items))) => items.push(s(v)),
                Some((_, slot)) => *slot = Value::Array(vec![s(v)]),
                None => d.push((key, Value::Array(vec![s(v)]))),
            }
        } else if let Some(k) = k.strip_suffix('#') {
            let digits = v.trim_start_matches('0');
            set(
                &mut d,
                k,
                Value::Int(if digits.is_empty() { "0" } else { digits }.into()),
            );
        } else if let Some(k) = k.strip_suffix('?') {
            set(&mut d, k, Value::Bool(v == "1"));
        } else {
            set(&mut d, k, s(v));
        }
    }
    dumps(&Value::Object(d))
}

/// `_changes_files`: each word as the daemon is sent it. A relative word
/// is made absolute from where you stand (`cwd`, physical) when that is
/// inside `base`; else it goes as given.
pub fn files(cwd: &str, base: &str, args: &[String]) -> Vec<String> {
    use crate::out::pylib::path::{join, normpath};
    let inside = !base.is_empty()
        && (cwd == base || cwd.starts_with(&format!("{}/", base.trim_end_matches('/'))));
    args.iter()
        .map(|a| {
            if a.starts_with('/') || !inside {
                return a.clone();
            }
            let mut out = normpath(&join(cwd, a));
            if a.ends_with('/') && !out.ends_with('/') {
                out.push('/');
            }
            out
        })
        .collect()
}

// ---------------------------------------------------------------------
// The error reader.

/// `_changes_err STATUS BODY`: its stderr and `_CH_ERR_RC` (1 for no
/// answer, else 3).
pub fn refusal(status: &str, body: &[u8], as_json: bool) -> Shown {
    let raw = String::from_utf8_lossy(body).into_owned();
    let mut code = s(match status {
        "400" => "bad_request",
        "401" => "unauthorized",
        "403" => "forbidden",
        "404" => "not_found",
        "405" => "method_not_allowed",
        "409" => "conflicted",
        "000" => "daemon_unreachable",
        _ => "error",
    });
    let mut hint = s("");
    match json::parse(&raw) {
        Ok(Value::Object(v)) => match get(&v, "error") {
            Some(Value::Object(e)) => {
                if let Some(c) = get(e, "code").filter(|c| py::truthy(c)) {
                    code = c.clone();
                }
                hint = get(e, "hint")
                    .filter(|h| py::truthy(h))
                    .or_else(|| get(e, "message").filter(|m| py::truthy(m)))
                    .cloned()
                    .unwrap_or_else(|| s(""));
            }
            e => {
                hint = match e {
                    Some(Value::Null) | None => s(""),
                    Some(e) => s(&py::str_of(e)),
                };
                if let Some(c) = get(&v, "code").filter(|c| py::truthy(c)) {
                    code = c.clone();
                }
            }
        },
        _ => {
            let stripped = raw.trim_matches(py_space);
            if !stripped.is_empty() {
                hint = s(&stripped.chars().take(300).collect::<String>());
            }
        }
    }
    if status == "000" {
        hint = s(concat!(
            "no answer from the ",
            k2_client::product_name!(),
            " daemon"
        ));
    }
    if !py::truthy(&hint) {
        hint = s(&format!("the daemon refused (HTTP {status})"));
    }
    let mut err = String::new();
    if !as_json {
        err.push_str(&format!("Error: {}\n", py::str_of(&hint)));
    }
    let number = status.trim_start_matches('0');
    let all_digits = !status.is_empty() && status.bytes().all(|b| b.is_ascii_digit());
    let obj = Value::Object(vec![(
        Text::from("error"),
        Value::Object(vec![
            (Text::from("code"), code),
            (Text::from("hint"), hint),
            (
                Text::from("status"),
                Value::Int(
                    if !all_digits || number.is_empty() {
                        "0"
                    } else {
                        number
                    }
                    .into(),
                ),
            ),
        ]),
    )]);
    err.push_str(&dumps(&obj));
    err.push('\n');
    Shown::err(
        err,
        if status == "000" {
            Exit::Error
        } else {
            Exit::Refused
        },
    )
}

// ---------------------------------------------------------------------
// The run.

fn help_page(notes: bool) -> &'static str {
    let key = if notes { "changes/notes" } else { "changes" };
    crate::text::help(key).expect("the changes help pages are embedded")
}

/// `k5 changes …`: `args` is the whole command line after `argv[0]`.
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
    let parsed = parse(&words);
    if let Ok((verb, _)) = &parsed {
        if matches!(verb, Verb::Help { .. }) {
            return Some(run_with(
                args,
                &words,
                hook_sock.as_deref(),
                settings,
                parsed,
            ));
        }
        if let Some(ws) = verb.ws() {
            let cdpath = std::env::var_os("CDPATH").is_some_and(|v| !v.is_empty());
            if !ws.is_empty()
                && Path::new(ws).is_dir()
                && (ws.starts_with('-') || (cdpath && cd_searches(ws)))
            {
                return None;
            }
        }
    }
    if prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(
        args,
        &words,
        hook_sock.as_deref(),
        settings,
        parsed,
    ))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    hook_sock: Option<&Path>,
    mut settings: Settings,
    parsed: Result<(Verb, bool), Die>,
) -> Exit {
    let offline = words.iter().any(|w| matches!(*w, "-h" | "--help"));
    let skip = words.first() == Some(&"help") || offline;
    let no_token = settings.auth == Auth::None;
    if settings.no_owner_fallback && no_token && !offline {
        return Gate::VirtualDesktop.stop();
    }
    if !skip && (settings.port_text.as_deref().is_none_or(str::is_empty) || no_token) {
        return Gate::CannotConnect.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let (verb, json) = match parsed {
        Ok(v) => v,
        Err(d) => return d.print(),
    };
    match &verb {
        Verb::Help { notes } => {
            return match out::stdout(help_page(*notes)) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Verb::Unknown { notes, word } => {
            let n = name();
            let line = if *notes {
                format!("Error: unknown {n} changes notes command: {word} (list, add, resolve, reopen, edit, delete, send)")
            } else {
                format!("Error: unknown {n} changes command: {word} (list, diff, stage, unstage, discard, commit, notes, open, init)")
            };
            out::stderr_line(&line);
            let _ = out::write_flushed(&mut std::io::stderr().lock(), help_page(*notes).as_bytes());
            return Exit::Usage;
        }
        _ => {}
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut run = Run {
        in_cell: hook_sock.is_some() && client.passport().is_some() && !client.force_tcp,
        client,
        closing: Closing::default(),
        project: settings.project.clone(),
        json,
    };
    let exit = match run.verb(verb) {
        Ok(()) => Exit::Ok,
        Err(e) => e,
    };
    let Run {
        mut client,
        closing,
        ..
    } = run;
    closing.flush(&mut client, exit);
    exit
}

/// What `_changes_target` set.
#[derive(Debug, Clone, Default)]
pub struct Target {
    /// `_CH_PATH`: the `path` sent (empty: the passport's own).
    pub path: String,
    /// `_CH_DIR`: that folder, physical, for file words.
    pub dir: String,
}

/// A split `cli_send_json` answer.
pub struct Sent {
    pub status: String,
    pub body: Vec<u8>,
}

pub struct Run {
    pub client: Client,
    pub closing: Closing,
    pub project: String,
    /// `_changes_in_cell`.
    pub in_cell: bool,
    /// `_CH_JSON`.
    pub json: bool,
}

/// The physical path of a folder (`cd <it> && pwd -P`), or nothing.
fn physical(dir: &str) -> String {
    std::fs::canonicalize(dir)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `pwd -P`.
pub fn physical_cwd() -> String {
    std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Run {
    fn die(&self, d: Die) -> Exit {
        Die {
            json: self.json,
            ..d
        }
        .print()
    }

    /// `cli_send_json`, split as `_changes_split` splits it. A POST that
    /// went out on the socket with no answer ends the command.
    pub fn send(&mut self, post: bool, route: &str, payload: &str) -> Result<Sent, Exit> {
        let passport = self.client.passport().is_some();
        match self.client.send_json(post, route, payload) {
            Ok(r) => {
                self.closing.answer(&r, passport);
                Ok(Sent {
                    status: format!("{:03}", r.status),
                    body: super::without_nuls(&r.body),
                })
            }
            Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
                out::stderr_line(&e.to_string());
                Err(Exit::Error)
            }
            Err(_) => Ok(Sent {
                status: "000".into(),
                body: Vec::new(),
            }),
        }
    }

    /// `_changes_call`: the body of a 200; anything else is the error
    /// reader's lines and its exit.
    pub fn call(&mut self, post: bool, route: &str, payload: &str) -> Result<Vec<u8>, Exit> {
        let r = self.send(post, route, payload)?;
        if r.status != "200" {
            return Err(refusal(&r.status, &r.body, self.json).print());
        }
        Ok(r.body)
    }

    /// `printf '%s\n' "$_CH_BODY"`.
    fn print_body(&self, body: &[u8]) -> Result<(), Exit> {
        let mut b = body.to_vec();
        b.push(b'\n');
        out::write_flushed(&mut std::io::stdout().lock(), &b)
            .map_err(|e| out::after_stdout_error(&e))
    }

    fn shown(&self, s: Shown) -> Result<(), Exit> {
        match s.print() {
            Exit::Ok => Ok(()),
            e => Err(e),
        }
    }

    /// `_changes_target <ws>`.
    pub fn target(&mut self, ws: &str) -> Target {
        if !ws.is_empty() {
            if Path::new(ws).is_dir() {
                return Target {
                    path: cd_pwd(&logical_pwd(), ws).unwrap_or_default(),
                    dir: physical(ws),
                };
            }
            let q = format!("q={}", quote(ws));
            let passport = self.client.passport().is_some();
            let got = match self.client.call(
                Channel::Tcp,
                "/cli/workspace/resolve",
                Call::Request(&[q.as_str()]),
            ) {
                Ok(r) => {
                    self.closing.answer(&r, passport);
                    render::resolved(&super::piped_body(&r))
                }
                Err(Error::NoAnswer(_)) => super::end_in_pipe(),
                Err(_) => String::new(),
            };
            let path = if got.is_empty() { ws.to_string() } else { got };
            return Target {
                dir: path.clone(),
                path,
            };
        }
        let proj_p = physical(&self.project);
        let top = git_top(&logical_pwd());
        let top_p = if top.is_empty() {
            String::new()
        } else {
            physical(&top)
        };
        if !top_p.is_empty() && !proj_p.is_empty() && top_p.starts_with(&format!("{proj_p}/")) {
            return Target {
                path: top,
                dir: top_p,
            };
        }
        Target {
            dir: if proj_p.is_empty() {
                self.project.clone()
            } else {
                proj_p
            },
            path: if self.in_cell {
                String::new()
            } else {
                self.project.clone()
            },
        }
    }

    fn verb(&mut self, verb: Verb) -> Result<(), Exit> {
        let n = name();
        match verb {
            Verb::Help { .. } | Verb::Unknown { .. } => unreachable!("answered before the run"),
            Verb::List { ws, base } => {
                let t = self.target(&ws);
                let mut query = q(&t.path);
                if !base.is_empty() {
                    query.push_str(&format!("base={}&", quote(&base)));
                }
                let b = self.call(false, "/cli/git/snapshot", trim_amp(&query))?;
                if self.json {
                    return self.print_body(&b);
                }
                let shown = render::list(&b);
                let not_repo = shown.exit == Exit::NotFound;
                let exit = if not_repo { Exit::Ok } else { shown.exit };
                self.shown(Shown { exit, ..shown })?;
                if not_repo {
                    return Err(self.die(Die {
                        exit: Exit::Refused,
                        ..die(
                            "not_a_repo",
                            format!("not a git repository (make one with: {n} changes init)"),
                        )
                    }));
                }
                Ok(())
            }
            Verb::Diff(o) => diff::run(self, o),
            Verb::Stage {
                unstage,
                ws,
                all,
                paths,
            } => {
                let t = self.target(&ws);
                if all {
                    self.call(true, "/cli/git/stage-all", &body(&t.path, Body::All))?;
                    let line = if self.json {
                        "{\"success\":true,\"staged\":\"all\"}\n"
                    } else {
                        "Staged all changes.\n"
                    };
                    return self.shown(Shown::out(line));
                }
                let fs = files(&physical_cwd(), &t.dir, &paths);
                let verb = if unstage { "unstage" } else { "stage" };
                let mut done = Vec::new();
                for (f, shown) in fs.iter().zip(&paths) {
                    let r = self.send(
                        true,
                        &format!("/cli/git/{verb}"),
                        &body(&t.path, Body::File(f)),
                    )?;
                    if r.status != "200" {
                        if !self.json {
                            out::stderr_line(&format!("({shown})"));
                        }
                        return Err(refusal(&r.status, &r.body, self.json).print());
                    }
                    done.push(shown.clone());
                    if !self.json {
                        let word = if unstage { "Unstaged" } else { "Staged" };
                        self.shown(Shown::out(format!("{word} {shown}\n")))?;
                    }
                }
                if self.json {
                    let key = if unstage { "unstaged" } else { "staged" };
                    let v = Value::Object(vec![
                        (Text::from("success"), Value::Bool(true)),
                        (
                            Text::from(key),
                            Value::Array(done.iter().map(|d| s(d)).collect()),
                        ),
                    ]);
                    return self.shown(Shown::out(dumps(&v) + "\n"));
                }
                Ok(())
            }
            Verb::Discard {
                ws,
                yes,
                staged,
                paths,
            } => {
                let t = self.target(&ws);
                let fs = files(&physical_cwd(), &t.dir, &paths);
                if !yes {
                    if !super::sim::is_tty(0) {
                        return Err(self.die(die(
                            "confirm_required",
                            format!("{n} changes discard needs --yes when not run on a terminal (untracked files and every copy it replaces go to the Trash)"),
                        )));
                    }
                    let what = if staged {
                        "back to the last commit (staged edits too)"
                    } else {
                        "back to the staged copy"
                    };
                    let _ = out::write_flushed(
                        &mut std::io::stderr().lock(),
                        format!("Discard changes to {} file(s)? Edited files go {what}; untracked files and the current copies go to the Trash. [y/N] ", fs.len()).as_bytes(),
                    );
                    let ans = super::sim::read_line();
                    if !matches!(ans.as_str(), "y" | "Y" | "yes" | "YES" | "Yes") {
                        out::stderr_line("Nothing discarded.");
                        return Err(Exit::Error);
                    }
                }
                let b = self.call(
                    true,
                    "/cli/git/discard",
                    &body(&t.path, Body::Discard { staged, files: &fs }),
                )?;
                if self.json {
                    self.print_body(&b)?;
                    return self.shown(render::discard_failed(&b));
                }
                self.shown(render::discard(&b))
            }
            Verb::Commit { ws, all, msg } => {
                let t = self.target(&ws);
                if all {
                    self.call(true, "/cli/git/stage-all", &body(&t.path, Body::All))?;
                }
                let query = q(&t.path);
                let b = self.call(false, "/cli/git/snapshot", trim_amp(&query))?;
                let staged = render::staged(&b);
                let staged = match staged {
                    Ok(v) => v,
                    Err(sh) => return Err(sh.print()),
                };
                if staged == "-1" {
                    return Err(self.die(Die {
                        exit: Exit::Refused,
                        ..die("not_a_repo", "not a git repository")
                    }));
                }
                if staged == "0" {
                    return Err(self.die(Die {
                        exit: Exit::Refused,
                        ..die(
                            "nothing_staged",
                            format!("nothing is staged to commit; stage files first ({n} changes stage <path>) or use --all"),
                        )
                    }));
                }
                let b = self.call(true, "/cli/git/commit", &body(&t.path, Body::Commit(&msg)))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::committed(&b))
            }
            Verb::NotesList { ws, status, file } => {
                let t = self.target(&ws);
                let mut query = q(&t.path);
                if !status.is_empty() {
                    query.push_str(&format!("status={}&", quote(&status)));
                }
                if !file.is_empty() {
                    let f = files(&physical_cwd(), &t.dir, &[file]);
                    query.push_str(&format!("file_path={}&", quote(&f[0])));
                }
                let b = self.call(false, "/cli/git/notes", trim_amp(&query))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::notes(&b))
            }
            Verb::NotesIds { verb, ws, ids } => {
                let t = self.target(&ws);
                let mut pairs: Vec<String> = ids.iter().map(|i| format!("ids[]={i}")).collect();
                let route = match verb {
                    IdsVerb::Delete => "/cli/git/notes/delete",
                    _ => "/cli/git/notes/resolve",
                };
                if verb == IdsVerb::Reopen {
                    pairs.push("resolved?=0".into());
                }
                let b = self.call(true, route, &json_body(&t.path, &pairs))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::ids(&b, verb))
            }
            Verb::NotesAdd {
                ws,
                file,
                side,
                start,
                end,
                msg,
            } => {
                let t = self.target(&ws);
                let f = files(&physical_cwd(), &t.dir, &[file]);
                let pairs = vec![
                    format!("filePath={}", f[0]),
                    format!("side={side}"),
                    format!("startLine#={start}"),
                    format!("endLine#={end}"),
                    format!("body={msg}"),
                ];
                let b = self.call(true, "/cli/git/notes/add", &json_body(&t.path, &pairs))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::added(&b))
            }
            Verb::NotesEdit { ws, id, msg } => {
                self.target(&ws);
                let pairs = vec![format!("id={id}"), format!("body={msg}")];
                let b = self.call(true, "/cli/git/notes/update", &json_body("", &pairs))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(Shown::out(format!("Updated note {id}\n")))
            }
            Verb::NotesSend { ws, wake, ids } => {
                let mut t = self.target(&ws);
                if t.path.is_empty() {
                    t.path = self.project.clone();
                }
                let mut pairs: Vec<String> = ids.iter().map(|i| format!("ids[]={i}")).collect();
                pairs.push(format!("wake?={}", u8::from(wake)));
                let b = self.call(true, "/cli/git/notes/send", &json_body(&t.path, &pairs))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::sent(&b))
            }
            Verb::Open { ws } => {
                let t = self.target(&ws);
                let b = self.call(true, "/cli/git/open-changes", &body(&t.path, Body::All))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::opened(&b))
            }
            Verb::Init { ws } => {
                let mut t = self.target(&ws);
                if ws.is_empty() {
                    t.path = if self.in_cell {
                        String::new()
                    } else {
                        self.project.clone()
                    };
                }
                let b = self.call(true, "/cli/git/init", &body(&t.path, Body::All))?;
                if self.json {
                    return self.print_body(&b);
                }
                self.shown(render::initialized(&b))
            }
        }
    }
}

/// `git -C "$PWD" rev-parse --show-toplevel` when git may run, its
/// trailing newlines dropped; nothing when it fails.
fn git_top(pwd: &Path) -> String {
    let Some(git) = daemon::usable_git() else {
        return String::new();
    };
    let Ok(out) = std::process::Command::new(git)
        .arg("-C")
        .arg(pwd)
        .args(["rev-parse", "--show-toplevel"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
    else {
        return String::new();
    };
    let mut t = String::from_utf8_lossy(&out.stdout).into_owned();
    while t.ends_with('\n') {
        t.pop();
    }
    t
}

#[cfg(test)]
mod tests;
