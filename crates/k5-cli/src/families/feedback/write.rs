//! The rest of `k5 feedback` (`tickets`): the family alone (its help), an
//! unknown verb, `ask` ([`super::ask`]), `template`, `comment`, `resolve`,
//! `assign` and `settings`, as the shell CLI's `feedback|tickets)` arm and
//! its `cmd_feedback_*` functions answer them (`cli/src/router.sh`,
//! `cli/src/feedback.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]). `template`
//!    never asks the daemon, so only the virtual desktop's guard stops it
//!    (`_skip_conn_gate`).
//! 2. The credential: `feedback` is an open tool, so in a session
//!    (`HOOK_SOCK` set) with the owner token on disk the shell sends the
//!    owner token over TCP, a passport or not (COMPAT-58,
//!    `_cli_apply_auth_for_verb`). `ask`, `comment`, `resolve` and `assign`
//!    carry it on purpose: the daemon's `/cli/feedback/*` doors are
//!    decided by it (`feedback_routes.rs`). This copies it
//!    ([`open_tool`]); `settings` reads with it too and writes with the
//!    session's passport (`cmd_feedback_settings`).
//! 3. The verb's words ([`Form::parse`]); a word it can't take is one line
//!    of JSON on stderr (`_feedback_err "usage" …`), exit 2, nothing sent.
//! 4. `comment`, `resolve` and `assign`: one `cli_post_json` over TCP
//!    (sent once; see [`crate::transport`]), `settings` a `cli_request` or
//!    a `cli_post_json`. `$(…)` drops the answer's NULs and trailing
//!    newlines. No answer: the transport's line, exit 1; an empty answer
//!    or any other failure: the `daemon_unreachable` line, exit 1. Then
//!    the shell's Python on the answer ([`comment_reader`] and the
//!    others), read from the body here.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! # Where this differs from the shell (a named difference, K2's sign-off)
//!
//! The shell hands the daemon's answer to its Python in the environment
//! (`RESP="$resp" python3 - …`, `cli/src/feedback.sh` `comment`,
//! `resolve`, `assign` and `settings`). Linux refuses one environment
//! string over 128 KiB (`MAX_ARG_STRLEN`), so there the shell prints
//! `python3: Argument list too long` and exits 126. The binary reads the
//! answer from the body and prints it as for a short one
//! (`tests/feedback_write_parity.rs`, the large-answer rows).

use super::ask::{self, Ctx};
use super::{at, at_or, code_exit, dumps, err_line, join, obj, s, short_text, str_of, PyStr};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{curl_port, get, py_iter, Raise, Shown};
use crate::out::json::{self, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::transport::urllib::Opener;
use crate::transport::{Auth, Call, Channel, Client, Error};
use k2_client::{cli_name, env_names};
use std::ffi::OsString;

/// The routes.
pub const COMMENT: &str = "/cli/feedback/comment";
pub const RESOLVE: &str = "/cli/feedback/resolve";
pub const ASSIGN: &str = "/cli/feedback/assign";
pub const SETTINGS: &str = "/cli/tickets/settings";

/// `k2 tickets template`'s brief skeleton (prd-ticket-html-brief-v1 H15),
/// on stdout.
pub const TEMPLATE: &str = "<h2>Problem</h2>
<p>What is wrong, in plain words. One short paragraph.</p>

<h2>What I tried</h2>
<ul>
  <li>The command or change, and what happened.</li>
  <li>Why that did not solve it.</li>
</ul>

<section class=\"k2-need\">
  <h2>What I need from you</h2>
  <p>The one decision or action you need, stated so it can be answered in a line.</p>
</section>

<h2>Options</h2>
<ol class=\"k2-options\">
  <li><strong>Option A</strong> \u{2014} what happens, and the cost.</li>
  <li><strong>Option B</strong> \u{2014} what happens, and the cost.</li>
</ol>

<h2>Context</h2>
<p class=\"k2-callout\">Anything the human must know before answering.</p>
<pre><code>Relevant log lines or command output.</code></pre>
<p>Screenshots go inline as &lt;img alt=\"what it shows\" src=\"data:image/png;base64,\u{2026}\"&gt;. Delete this line if you have none.</p>
";

/// The template's next step, on stderr.
pub const TEMPLATE_NEXT: &str = concat!(
    "Next: ",
    cli_name!(),
    " tickets ask \"<title>\" --html brief.html --assign <user>  (a user on this server: ",
    cli_name!(),
    " connections list --users; the owner may require it: ",
    cli_name!(),
    " tickets settings)"
);

/// The lines of the Python blocks that raise (`<stdin>`).
pub mod line {
    pub const C_LOAD: u32 = 3;
    pub const C_ERR: u32 = 4;
    pub const C_CODE: u32 = 9;
    pub const C_FILES: u32 = 16;
    pub const R_FILES: u32 = 16;
    pub const A_LOAD: u32 = 2;
    pub const A_ERR: u32 = 3;
    pub const A_CODE: u32 = 8;
    pub const A_WARN: u32 = 12;
    pub const A_JOIN: u32 = 21;
    pub const S_ERR: u32 = 7;
    pub const S_CODE: u32 = 12;
    pub const S_WHO: u32 = 22;
    pub const S_BRIEF: u32 = 26;
    pub const S_ASSIGNEE: u32 = 27;
}

/// A traceback as `python3 -` prints it: the block's frames are
/// `<stdin>`'s, `_ticket_files_check_py`'s (run by `exec`) `<string>`'s.
pub fn traceback_stdin(r: &Raise) -> String {
    let mut t = String::from("Traceback (most recent call last):\n");
    for (line, name) in &r.frames {
        let file = if matches!(*name, "ticket_file_lines" | "ticket_size") {
            "<string>"
        } else {
            "<stdin>"
        };
        t.push_str(&format!("  File \"{file}\", line {line}, in {name}\n"));
    }
    t.push_str(&r.exception);
    t.push('\n');
    t
}

/// What a `python3 -` block printed so far.
#[derive(Default)]
pub struct PyOut {
    pub stdout: String,
    pub stderr: String,
}

impl PyOut {
    pub fn line(&mut self, text: &str) {
        self.stdout.push_str(text);
        self.stdout.push('\n');
    }

    pub fn done(self, exit: Exit) -> Shown {
        Shown {
            stdout: self.stdout,
            stderr: self.stderr,
            exit,
        }
    }

    pub fn raised(self, r: &Raise) -> Shown {
        Shown {
            stdout: self.stdout,
            stderr: self.stderr + &traceback_stdin(r),
            exit: Exit::Error,
        }
    }
}

/// `ticket_files_check(sent, d)`: the `server_too_old` line when files
/// were sent and the answer has no `attachments` key.
pub fn ticket_files_check(sent: bool, d: &Value) -> Option<String> {
    let Value::Object(pairs) = d else {
        return None;
    };
    (sent && get(pairs, "attachments").is_none()).then(|| {
        err_line(
            "server_too_old",
            concat!(
                "this server's ",
                k2_client::product_name!(),
                " doesn't take ticket files (needs 0.46.6)"
            ),
        ) + "\n"
    })
}

// ---------------------------------------------------------------------
// `_thread_attachments_json`

/// `os.path.expanduser(p)`, `~` and `~/…` from `HOME`. A `~name` is kept
/// as it is.
fn expanduser(p: &str, home: Option<&str>) -> String {
    let Some(rest) = p.strip_prefix('~') else {
        return p.to_string();
    };
    let i = rest.find('/').unwrap_or(rest.len());
    if i != 0 {
        return p.to_string();
    }
    let Some(home) = home else {
        return p.to_string();
    };
    let joined = format!("{}{rest}", home.trim_end_matches('/'));
    if joined.is_empty() {
        "/".into()
    } else {
        joined
    }
}

/// `os.path.join(a, b)`.
fn join_posix(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `os.path.normpath(p)`.
pub fn normpath(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let slashes = if p.starts_with("//") && !p.starts_with("///") {
        2
    } else if p.starts_with('/') {
        1
    } else {
        0
    };
    let mut comps: Vec<&str> = Vec::new();
    for c in p.split('/') {
        if c.is_empty() || c == "." {
            continue;
        }
        if c != ".." || (slashes == 0 && comps.is_empty()) || comps.last() == Some(&"..") {
            comps.push(c);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let out = format!("{}{}", "/".repeat(slashes), comps.join("/"));
    if out.is_empty() {
        ".".into()
    } else {
        out
    }
}

fn exists(p: &str) -> bool {
    std::fs::metadata(p).is_ok()
}

/// `int(digits)`.
fn int_text(digits: &str) -> String {
    let t = digits.trim_start_matches('0');
    if t.is_empty() {
        "0".into()
    } else {
        t.into()
    }
}

/// `re.match(r"^(.+?):(\d+)(?:-(\d+))?$", spec)`: the path, the line and
/// the end line. `$` also matches before a last newline; `.` takes no
/// newline.
pub fn spec_line(spec: &str) -> Option<(String, String, Option<String>)> {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    for (at, _) in spec.match_indices(':') {
        let head = &spec[..at];
        if head.is_empty() || head.contains('\n') {
            continue;
        }
        let tail = &spec[at + 1..];
        let tail = tail.strip_suffix('\n').unwrap_or(tail);
        let (line, end) = match tail.split_once('-') {
            Some((l, e)) => (l, Some(e)),
            None => (tail, None),
        };
        if digits(line) && end.is_none_or(digits) {
            return Some((head.to_string(), int_text(line), end.map(int_text)));
        }
    }
    None
}

/// `_thread_attachments_json <spec>…` (`cli/src/browser.sh`), as the
/// value `json.loads` gives back: each spec a path (made absolute when it
/// is there, from `cwd`, the shell's `$PWD`), and its line and end line
/// when the spec is `path:line[-end]` and no file is named the whole spec.
pub fn attachments(specs: &[String], cwd: &str, home: Option<&str>) -> Value {
    let here = |p: &str| join_posix(cwd, &expanduser(p, home));
    Value::Array(
        specs
            .iter()
            .map(|spec| {
                let (mut path, mut line, mut end) = (spec.clone(), None, None);
                if !exists(&here(spec)) {
                    if let Some((p, l, e)) = spec_line(spec) {
                        path = p;
                        line = Some(l);
                        end = e;
                    }
                }
                if exists(&here(&path)) {
                    path = normpath(&here(&path));
                }
                let mut pairs = vec![("path", s(&path))];
                if let Some(l) = line {
                    pairs.push(("line", Value::Int(l)));
                }
                if let Some(e) = end {
                    pairs.push(("lineEnd", Value::Int(e)));
                }
                obj(pairs)
            })
            .collect(),
    )
}

// ---------------------------------------------------------------------
// The words

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    /// `feedback` alone (or an empty verb): its help.
    Help,
    /// A verb the family has no arm for.
    Unknown(String),
    Ask(Result<ask::Words, String>),
    /// `template` and the usage line its words stop at, if any.
    Template(Result<(), String>),
    Comment(Result<Comment, String>),
    Resolve(Result<Resolve, String>),
    Assign(Result<Assign, String>),
    Settings(Result<SettingsWords, String>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub text: String,
    pub json: bool,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolve {
    pub id: String,
    pub json: bool,
    pub answered: Option<String>,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Assign {
    pub id: String,
    pub json: bool,
    pub clear: bool,
    pub users: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsWords {
    pub json: bool,
    pub brief: String,
    pub assignee: String,
    pub default: Option<String>,
}

fn usage(hint: &str) -> String {
    err_line("usage", hint)
}

const FILE_NEEDS: &str = "--file requires a path (path[:line[-end]])";
const TEN_FILES: &str = "at most 10 files per message";

impl Form {
    /// The words after `feedback`.
    pub fn parse(words: &[&str]) -> Form {
        let verb = words.first().copied().unwrap_or("");
        let rest = words.get(1..).unwrap_or_default();
        match verb {
            "" => Form::Help,
            "ask" => Form::Ask(ask::parse(rest)),
            "template" => Form::Template(if rest.is_empty() {
                Ok(())
            } else {
                Err(usage(concat!(
                    "template takes no arguments \u{2014} usage: ",
                    cli_name!(),
                    " tickets template > brief.html"
                )))
            }),
            "comment" => Form::Comment(parse_comment(rest)),
            "resolve" => Form::Resolve(parse_resolve(rest)),
            "assign" => Form::Assign(parse_assign(rest)),
            "settings" => Form::Settings(parse_settings(rest)),
            other => Form::Unknown(other.to_string()),
        }
    }
}

pub fn parse_comment(rest: &[&str]) -> Result<Comment, String> {
    let mut c = Comment::default();
    let mut i = 0;
    while let Some(&a) = rest.get(i) {
        match a {
            "--json" => c.json = true,
            "--file" => {
                c.files
                    .push(rest.get(i + 1).ok_or_else(|| usage(FILE_NEEDS))?.to_string());
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(&format!(
                    "unknown flag '{f}' \u{2014} usage: {} tickets comment <id> [<text>] [--file path[:line]]\u{2026} [--json]",
                    k2_client::brand::CLI_NAME
                )))
            }
            w if c.id.is_empty() => c.id = w.to_string(),
            w if c.text.is_empty() => c.text = w.to_string(),
            w => {
                return Err(usage(&format!(
                    "unexpected extra argument '{w}' \u{2014} quote the comment text"
                )))
            }
        }
        i += 1;
    }
    if c.id.is_empty() || (c.text.is_empty() && c.files.is_empty()) {
        return Err(usage(concat!(
            "usage: ",
            cli_name!(),
            " tickets comment <id> \"<text>\" (the text is optional with --file)"
        )));
    }
    if c.files.len() > 10 {
        return Err(usage(TEN_FILES));
    }
    Ok(c)
}

const ANSWERED_NEEDS: &str = concat!(
    "--answered requires the agreed outcome \u{2014} ",
    cli_name!(),
    " tickets resolve <id> --answered \"<outcome>\""
);

/// `tr -d '[:space:]'` left nothing.
fn blank(t: &str) -> bool {
    t.bytes()
        .all(|b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
}

pub fn parse_resolve(rest: &[&str]) -> Result<Resolve, String> {
    let mut r = Resolve::default();
    let mut i = 0;
    while let Some(&a) = rest.get(i) {
        match a {
            "--json" => r.json = true,
            "--file" => {
                r.files
                    .push(rest.get(i + 1).ok_or_else(|| usage(FILE_NEEDS))?.to_string());
                i += 1;
            }
            "--answered" => {
                r.answered = Some(
                    rest.get(i + 1)
                        .ok_or_else(|| usage(ANSWERED_NEEDS))?
                        .to_string(),
                );
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(&format!(
                    "unknown flag '{f}' \u{2014} usage: {} tickets resolve <id> [--answered \"<outcome>\" [--file path[:line]]\u{2026}] [--json]",
                    k2_client::brand::CLI_NAME
                )))
            }
            w if r.id.is_empty() => r.id = w.to_string(),
            w => {
                return Err(usage(&format!(
                    "unexpected extra argument '{w}' \u{2014} usage: {} tickets resolve <id>",
                    k2_client::brand::CLI_NAME
                )))
            }
        }
        i += 1;
    }
    if r.id.is_empty() {
        return Err(usage(
            "resolve requires an <id> (from ask/list; a short unique prefix works)",
        ));
    }
    if r.answered.as_deref().is_some_and(blank) {
        return Err(usage(ANSWERED_NEEDS));
    }
    if !r.files.is_empty() && r.answered.is_none() {
        return Err(usage(concat!(
            "--file needs --answered (files ride a message; use ",
            cli_name!(),
            " tickets comment <id> --file \u{2026})"
        )));
    }
    if r.files.len() > 10 {
        return Err(usage(TEN_FILES));
    }
    Ok(r)
}

/// `IFS=',' read -r -a _parts <<< "$v"`, each part with its spaces
/// trimmed, the empty ones left out: the first line of `v` only.
fn to_users(v: &str) -> Vec<String> {
    let first = v.split('\n').next().unwrap_or_default();
    let space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\u{0b}' | '\u{0c}' | '\r');
    first
        .split(',')
        .map(|u| u.trim_matches(space))
        .filter(|u| !u.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn parse_assign(rest: &[&str]) -> Result<Assign, String> {
    let mut a = Assign::default();
    let mut i = 0;
    while let Some(&w) = rest.get(i) {
        match w {
            "--json" => a.json = true,
            "--clear" => a.clear = true,
            "--to" | "--assign" => {
                let v = rest
                    .get(i + 1)
                    .ok_or_else(|| usage("--to requires a comma-separated username list"))?;
                a.users.extend(to_users(v));
                i += 1;
            }
            f if f.starts_with('-') => {
                return Err(usage(&format!(
                    "unknown flag '{f}' \u{2014} usage: {} tickets assign <id> <user> [user\u{2026}] | --to u1,u2 | --clear [--json]",
                    k2_client::brand::CLI_NAME
                )))
            }
            u if a.id.is_empty() => a.id = u.to_string(),
            u => a.users.push(u.to_string()),
        }
        i += 1;
    }
    if a.id.is_empty() {
        return Err(usage(
            "assign requires an <id> (from ask/list; a short unique prefix works)",
        ));
    }
    if a.clear {
        a.users.clear();
    } else if a.users.is_empty() {
        return Err(usage(concat!(
            "assign requires at least one username (or --clear). Prefer: ",
            cli_name!(),
            " tickets assign <id> owner  OR  --to owner,julie"
        )));
    }
    Ok(a)
}

pub fn parse_settings(rest: &[&str]) -> Result<SettingsWords, String> {
    let mut st = SettingsWords::default();
    let mut i = 0;
    while let Some(&w) = rest.get(i) {
        match w {
            "--json" => st.json = true,
            "--brief" | "--assignee-required" => {
                let v = rest
                    .get(i + 1)
                    .ok_or_else(|| usage(&format!("{w} requires off, warn or require")))?;
                if !matches!(*v, "off" | "warn" | "require") {
                    return Err(usage(&format!(
                        "{w} must be off, warn or require (got '{v}')"
                    )));
                }
                if w == "--brief" {
                    st.brief = v.to_string();
                } else {
                    st.assignee = v.to_string();
                }
                i += 1;
            }
            "--default-assignee" => {
                let v = rest.get(i + 1).ok_or_else(|| {
                    usage("--default-assignee requires a user (owner or a Connect user) or none")
                })?;
                st.default = Some(v.to_string());
                i += 1;
            }
            other => {
                return Err(usage(&format!(
                    "unknown argument '{other}' \u{2014} usage: {} tickets settings [--brief off|warn|require] [--assignee-required off|warn|require] [--default-assignee <user|none>] [--json]",
                    k2_client::brand::CLI_NAME
                )))
            }
        }
        i += 1;
    }
    Ok(st)
}

// ---------------------------------------------------------------------
// The run

/// The settings the shell's Python blocks read from the environment.
fn python_names() -> Vec<&'static str> {
    [
        env_names::AGENT_NAME,
        env_names::SESSION_ID,
        env_names::CELL,
        env_names::API_CELL,
        env_names::TEST_FEEDBACK_DETECT,
    ]
    .concat()
    .into_iter()
    .chain(["CLAUDE_SESSION_ID", "CLAUDE_CODE_SESSION_ID", "HOME", "PWD"])
    .collect()
}

/// `$(cat file)` of the owner token files the shell's prelude reads
/// (`heartbeat.token`, then `daemon.token`): its NULs and trailing
/// newlines dropped, the first that is not empty.
fn disk_owner(home: &std::path::Path) -> Option<String> {
    ["heartbeat.token", "daemon.token"].iter().find_map(|f| {
        let raw = std::fs::read(home.join(f)).ok()?;
        let held = super::super::connections::held(&raw);
        let t = String::from_utf8_lossy(&held).into_owned();
        (!t.is_empty()).then_some(t)
    })
}

/// `_cli_apply_auth_for_verb feedback …` for an open tool: in a session
/// (`HOOK_SOCK` set) with the owner token on disk, the owner token, a
/// passport or not (COMPAT-58). With a passport the shell sends no
/// `X-K5-Owner-Source` (its `SCOPED_TOKEN` is set).
pub fn open_tool(settings: &Settings) -> Settings {
    let mut s = settings.clone();
    if s.session && !s.no_owner_fallback && matches!(s.auth, Auth::Passport { .. }) {
        if let Some(t) = disk_owner(&s.home) {
            s.auth = Auth::Owner(t);
            s.owner_from_disk = false;
        }
    }
    s
}

/// `k5 feedback <words>` for every form but `list` and `show`. `None`: the
/// shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString], words: &[&str]) -> Option<Exit> {
    // Help is the front's: a help page, or the shell's.
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let form = Form::parse(words);
    if !python::family_native(python::here()) {
        return None;
    }
    // What the Python reads from the environment must be text.
    for name in python_names() {
        if let Some(v) = std::env::var_os(name) {
            v.to_str()?;
        }
    }
    let mktemp = match &form {
        Form::Ask(Ok(w)) if w.reads_stdin() => Some(super::super::update::shell_program(
            &crate::offline::Env::current(),
            "mktemp",
        )?),
        _ => None,
    };
    let cwd = match &form {
        Form::Ask(Ok(ask::Words { files, .. }))
        | Form::Comment(Ok(Comment { files, .. }))
        | Form::Resolve(Ok(Resolve { files, .. }))
            if !files.is_empty() =>
        {
            let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
            pwd.to_str()?.to_string()
        }
        _ => String::new(),
    };
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    let template = matches!(form, Form::Template(_));
    match prelude::gate(&settings) {
        Some(g @ Gate::VirtualDesktop) => return Some(g.stop()),
        Some(g @ Gate::CannotConnect) if !template => return Some(g.stop()),
        _ if template => {}
        _ => {
            // A port curl won't take is the shell's.
            curl_port(settings.port_text.as_deref()?)?;
        }
    }
    Some(run_form(args, form, settings, mktemp, cwd))
}

fn help() -> &'static str {
    crate::text::help("feedback").expect("cli/text/help/feedback.txt is embedded")
}

fn run_form(
    args: &[OsString],
    form: Form,
    mut settings: Settings,
    mktemp: Option<std::path::PathBuf>,
    cwd: String,
) -> Exit {
    match form {
        Form::Help => return Shown::out(help()).print(),
        Form::Unknown(verb) => {
            return Shown::err(
                format!("Unknown feedback subcommand: {verb}\n{}", help()),
                Exit::Usage,
            )
            .print()
        }
        Form::Template(Err(l)) => return stop(&l),
        Form::Template(Ok(())) => {
            return Shown {
                stdout: TEMPLATE.into(),
                stderr: format!("{TEMPLATE_NEXT}\n"),
                exit: Exit::Ok,
            }
            .print()
        }
        _ => {}
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let would_refuse = passport::current(args).would_refuse();
    let open = open_tool(&settings);
    let mut client = match open.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let env = |n: &str| std::env::var(n).ok();
    let agent = env_names::first_nonempty_in(env_names::AGENT_NAME, env)
        .map(|(_, v)| v)
        .unwrap_or_default();
    let author = if agent.is_empty() {
        basename(&settings.project)
    } else {
        agent.clone()
    };
    let home = std::env::var("HOME").ok();
    let mut closing = Closing::default();
    let exit = match form {
        Form::Ask(words) => {
            let mut opener = Opener::from_client(&client);
            if let Some(p) = &settings.port_text {
                opener.netloc = format!("{}:{p}", settings.host);
            }
            let ctx = Ctx {
                opener,
                project: settings.project.clone(),
                agent,
                session: ask::detect_session(env),
                detect_only: env_names::first_nonempty_in(env_names::TEST_FEEDBACK_DETECT, env)
                    .is_some_and(|(_, v)| v == "1"),
                mktemp,
                cwd,
                home,
            };
            // Python's urllib notes nothing for the EXIT trap.
            return ctx.ask(words);
        }
        Form::Comment(Err(l)) | Form::Resolve(Err(l)) | Form::Assign(Err(l)) => return stop(&l),
        Form::Settings(Err(l)) => return stop(&l),
        Form::Comment(Ok(c)) => {
            let mut body = vec![
                ("id", s(&c.id)),
                ("body", s(&c.text)),
                ("author", s(&author)),
            ];
            if !c.files.is_empty() {
                body.push(("attachments", attachments(&c.files, &cwd, home.as_deref())));
            }
            let sent = !c.files.is_empty();
            match post(&mut client, &mut closing, COMMENT, &dumps(&obj(body))) {
                Ok(raw) => comment_reader(&raw, c.json, sent).print(),
                Err(e) => e,
            }
        }
        Form::Resolve(Ok(r)) => {
            let mut body = vec![("id", s(&r.id))];
            if let Some(a) = &r.answered {
                body.push(("status", s("answered")));
                body.push(("answer", s(a)));
                body.push(("author", s(&author)));
            }
            if !r.files.is_empty() {
                body.push(("attachments", attachments(&r.files, &cwd, home.as_deref())));
            }
            let sent = !r.files.is_empty();
            match post(&mut client, &mut closing, RESOLVE, &dumps(&obj(body))) {
                Ok(raw) => resolve_reader(&raw, r.json, sent).print(),
                Err(e) => e,
            }
        }
        Form::Assign(Ok(a)) => {
            let joined = a.users.join("\n");
            let users: Vec<Value> = PyStr::from(joined.as_str())
                .splitlines()
                .into_iter()
                .filter(|u| {
                    !u.render()
                        .trim_matches(crate::families::py_space)
                        .is_empty()
                })
                .map(|u| s(&u.render()))
                .collect();
            let body = obj(vec![("id", s(&a.id)), ("usernames", Value::Array(users))]);
            match post(&mut client, &mut closing, ASSIGN, &dumps(&body)) {
                Ok(raw) => assign_reader(&raw, a.json).print(),
                Err(e) => e,
            }
        }
        Form::Settings(Ok(st)) => {
            let changed = !st.brief.is_empty() || !st.assignee.is_empty() || st.default.is_some();
            let got = if changed {
                let mut body = Vec::new();
                if !st.brief.is_empty() {
                    body.push(("briefPolicy", s(&st.brief)));
                }
                if !st.assignee.is_empty() {
                    body.push(("assigneePolicy", s(&st.assignee)));
                }
                if let Some(d) = &st.default {
                    body.push(("defaultAssignee", s(d)));
                }
                // In a session: the passport, never the owner token.
                client = match settings.client(would_refuse) {
                    Ok(c) => c,
                    Err(e) => {
                        out::stderr_line(&format!("Error: {e}"));
                        return Exit::Error;
                    }
                };
                post(&mut client, &mut closing, SETTINGS, &dumps(&obj(body)))
            } else {
                answer(&mut client, &mut closing, SETTINGS, Call::Request(&[]))
            };
            match got {
                Ok(raw) => settings_reader(&raw, st.json, changed).print(),
                Err(e) => e,
            }
        }
        Form::Help | Form::Unknown(_) | Form::Template(_) => unreachable!("answered above"),
    };
    closing.flush(&mut client, exit);
    exit
}

/// `basename "$PROJECT"`.
fn basename(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        return if p.is_empty() {
            String::new()
        } else {
            "/".into()
        };
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

fn stop(line: &str) -> Exit {
    out::stderr_line(line);
    Exit::Usage
}

fn unreachable_line() -> Exit {
    Shown::err(
        err_line("daemon_unreachable", super::UNREACHABLE) + "\n",
        Exit::Error,
    )
    .print()
}

/// `resp=$(cli_post_json <ep> "$body") || resp=""`.
fn post(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    body: &str,
) -> Result<Vec<u8>, Exit> {
    answer(client, closing, endpoint, Call::PostJson(body.as_bytes()))
}

/// `resp=$(<call>) || resp=""` over TCP: what `$(…)` held, or the exit
/// once the line for no answer (the transport's) or none at all
/// (`daemon_unreachable`) is printed.
fn answer(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    call: Call<'_>,
) -> Result<Vec<u8>, Exit> {
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, endpoint, call) {
        Ok(r) => {
            closing.answer(&r, passport);
            let raw = super::super::connections::held(&r.body);
            if raw.is_empty() {
                Err(unreachable_line())
            } else {
                Ok(raw)
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Err(Exit::Error)
        }
        Err(_) => Err(unreachable_line()),
    }
}

// ---------------------------------------------------------------------
// The readers

/// `json.loads(os.environ["RESP"])`, or what it raises at `line`.
fn load(raw: &[u8], line: u32) -> Result<Value, Raise> {
    let text = String::from_utf8_lossy(raw);
    json::parse(&text).map_err(|e| match e {
        json::Error::IntTooLong { .. } => Raise::at(
            line,
            "ValueError: Exceeds the limit (4300 digits) for integer string conversion",
        ),
        other => Raise::at(
            line,
            format!(
                "json.decoder.JSONDecodeError: {}",
                decode_why(&text, &other)
            ),
        ),
    })
}

fn decode_why(text: &str, e: &json::Error) -> String {
    if text.trim_matches(crate::families::py_space).is_empty() {
        "Expecting value: line 1 column 1 (char 0)".into()
    } else {
        e.to_string()
    }
}

/// The blocks' error branch: `err = d.get("error")` (raises at `get_line`
/// on what is not a dict); when it is not `None`, the error as JSON on
/// stderr and the exit `exits` gives its code (`err.get("code")` raises at
/// `code_line` on what is not a dict).
fn daemon_error(
    o: &mut PyOut,
    d: &Value,
    get_line: u32,
    code_line: u32,
    exits: fn(Option<&Value>) -> Exit,
) -> Result<Option<Exit>, Raise> {
    let Value::Object(pairs) = d else {
        return Err(Raise::no_attribute(get_line, d, "get"));
    };
    let Some(err) = get(pairs, "error").filter(|e| **e != Value::Null) else {
        return Ok(None);
    };
    let err = match err {
        Value::Str(_) => obj(vec![("code", s("error")), ("hint", err.clone())]),
        other => other.clone(),
    };
    o.stderr
        .push_str(&dumps(&obj(vec![("error", err.clone())])));
    o.stderr.push('\n');
    match &err {
        Value::Object(p) => Ok(Some(exits(get(p, "code")))),
        other => Err(Raise::no_attribute(code_line, other, "get")),
    }
}

fn shown(o: PyOut, r: Result<Exit, Raise>) -> Shown {
    match r {
        Ok(exit) => o.done(exit),
        Err(r) => o.raised(&r),
    }
}

/// `comment`'s Python on the answer.
pub fn comment_reader(raw: &[u8], as_json: bool, sent_files: bool) -> Shown {
    let mut o = PyOut::default();
    let r = comment_lines(raw, as_json, sent_files, &mut o);
    shown(o, r)
}

fn comment_lines(raw: &[u8], as_json: bool, sent: bool, o: &mut PyOut) -> Result<Exit, Raise> {
    let d = load(raw, line::C_LOAD)?;
    if let Some(exit) = daemon_error(o, &d, line::C_ERR, line::C_CODE, code_exit)? {
        return Ok(exit);
    }
    if let Some(e) = ticket_files_check(sent, &d) {
        o.stderr.push_str(&e);
        return Ok(Exit::Error);
    }
    if as_json {
        o.line(&dumps(&d));
        return Ok(Exit::Ok);
    }
    o.line(&format!(
        "Added comment to {}.",
        short_text(&at_or(&d, "id", s("")))
    ));
    let atts = at_or(&d, "attachments", Value::Null);
    for l in super::ticket_file_lines_from(&atts, &Value::Null, "  ", line::C_FILES)? {
        o.line(&l);
    }
    Ok(Exit::Ok)
}

/// `resolve`'s Python on the answer.
pub fn resolve_reader(raw: &[u8], as_json: bool, sent_files: bool) -> Shown {
    let mut o = PyOut::default();
    let r = resolve_lines(raw, as_json, sent_files, &mut o);
    shown(o, r)
}

fn resolve_lines(raw: &[u8], as_json: bool, sent: bool, o: &mut PyOut) -> Result<Exit, Raise> {
    let d = load(raw, line::C_LOAD)?;
    if let Some(exit) = daemon_error(o, &d, line::C_ERR, line::C_CODE, code_exit)? {
        return Ok(exit);
    }
    if let Some(e) = ticket_files_check(sent, &d) {
        o.stderr.push_str(&e);
        return Ok(Exit::Error);
    }
    let short = short_text(&at_or(&d, "id", s("")));
    if as_json {
        o.line(&dumps(&d));
    } else if at(&d, "status") == Some(&s("answered")) {
        o.line(&format!("Marked {short} answered."));
        let atts = at_or(&d, "attachments", Value::Null);
        for l in super::ticket_file_lines_from(&atts, &Value::Null, "  ", line::R_FILES)? {
            o.line(&l);
        }
    } else {
        o.line(&format!("Resolved {short}."));
    }
    Ok(Exit::Ok)
}

/// `assign`'s Python on the answer.
pub fn assign_reader(raw: &[u8], as_json: bool) -> Shown {
    let mut o = PyOut::default();
    let r = assign_lines(raw, as_json, &mut o);
    shown(o, r)
}

fn assign_lines(raw: &[u8], as_json: bool, o: &mut PyOut) -> Result<Exit, Raise> {
    let d = load(raw, line::A_LOAD)?;
    if let Some(exit) = daemon_error(o, &d, line::A_ERR, line::A_CODE, code_exit)? {
        return Ok(exit);
    }
    let warnings = at(&d, "warnings")
        .filter(|v| py::truthy(v))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    for w in py_iter(&warnings, line::A_WARN)? {
        if let Value::Object(_) = w {
            let text = |key: &str, default: &str| {
                at(&w, key)
                    .filter(|v| py::truthy(v))
                    .map_or_else(|| default.to_string(), str_of)
            };
            o.stderr.push_str(&dumps(&obj(vec![(
                "warning",
                obj(vec![
                    ("code", s(&text("code", "warning"))),
                    ("hint", s(&text("hint", ""))),
                ]),
            )])));
            o.stderr.push('\n');
        }
    }
    if as_json {
        o.line(&dumps(&d));
        return Ok(Exit::Ok);
    }
    let assignees = at(&d, "assignees")
        .filter(|v| py::truthy(v))
        .cloned()
        .unwrap_or(Value::Array(Vec::new()));
    let short = short_text(&at_or(&d, "id", s("")));
    if py::truthy(&assignees) {
        let names = join(&assignees, ", ").map_err(|e| Raise::at(line::A_JOIN, e))?;
        o.line(&format!("Assigned {short} \u{2192} {names}."));
    } else {
        o.line(&format!("Cleared assignees on {short}."));
    }
    Ok(Exit::Ok)
}

/// The exit for `settings`' error codes.
fn settings_exit(code: Option<&Value>) -> Exit {
    match code.and_then(|c| match c {
        Value::Str(t) => t.as_str(),
        _ => None,
    }) {
        Some("owner_only" | "role_required" | "forbidden") => Exit::Refused,
        Some("usage" | "bad_value" | "bad_request" | "unknown_user") => Exit::Usage,
        _ => Exit::Error,
    }
}

/// `settings`' Python on the answer.
pub fn settings_reader(raw: &[u8], as_json: bool, changed: bool) -> Shown {
    let mut o = PyOut::default();
    let r = settings_lines(raw, as_json, changed, &mut o);
    shown(o, r)
}

/// `labels.get(v, v)`.
fn label(v: &Value, at_line: u32) -> Result<String, Raise> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(Raise::at(
            at_line,
            format!("TypeError: unhashable type: '{}'", py::type_name(v)),
        )),
        Value::Str(t) => Ok(match t.as_str() {
            Some("off") => "off".into(),
            Some("warn") => "warn (file it and say so)".into(),
            Some("require") => "require (refuse without)".into(),
            _ => str_of(v),
        }),
        other => Ok(str_of(other)),
    }
}

fn settings_lines(raw: &[u8], as_json: bool, changed: bool, o: &mut PyOut) -> Result<Exit, Raise> {
    let text = String::from_utf8_lossy(raw);
    let Ok(d) = json::parse(&text) else {
        o.stderr.push_str(&err_line(
            "bad_response",
            concat!(
                "the daemon did not answer with JSON (older daemon? Settings \u{2192} Tickets needs ",
                k2_client::product_name!(),
                " 0.45.4)"
            ),
        ));
        o.stderr.push('\n');
        return Ok(Exit::Error);
    };
    if let Some(exit) = daemon_error(o, &d, line::S_ERR, line::S_CODE, settings_exit)? {
        return Ok(exit);
    }
    if as_json {
        o.line(&dumps(&d));
        return Ok(Exit::Ok);
    }
    let default = at_or(&d, "defaultAssignee", Value::Null);
    let mut who = if py::truthy(&default) {
        default.clone()
    } else {
        s("none")
    };
    if py::truthy(&default) && at(&d, "defaultAssigneeKnown") == Some(&Value::Bool(false)) {
        const MORE: &str = " (not a user here any more: not applied)";
        who = match who {
            Value::Str(t) => {
                let mut cps = t.code_points();
                cps.extend(MORE.chars().map(u32::from));
                Value::Str(Text::from(PyStr(cps).render()))
            }
            Value::Array(mut items) => {
                items.extend(MORE.chars().map(|c| s(&c.to_string())));
                Value::Array(items)
            }
            other => {
                return Err(Raise::at(
                    line::S_WHO,
                    format!(
                        "TypeError: unsupported operand type(s) for +=: '{}' and 'str'",
                        py::type_name(&other)
                    ),
                ))
            }
        };
    }
    if changed {
        o.line("Saved.");
    }
    o.line("Ticket rules (Settings \u{2192} Tickets)");
    let brief = label(&at_or(&d, "briefPolicy", Value::Null), line::S_BRIEF)?;
    o.line(&format!("  Require an HTML brief:  {brief}"));
    let assignee = label(&at_or(&d, "assigneePolicy", Value::Null), line::S_ASSIGNEE)?;
    o.line(&format!("  Require an assignee:    {assignee}"));
    o.line(&format!("  Default assignee:       {}", str_of(&who)));
    if !at(&d, "canEdit").is_some_and(py::truthy) {
        o.line("Only the owner or an admin can change these.");
    }
    Ok(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normpath_as_python() {
        for (p, want) in [
            ("/a/./b/../c", "/a/c"),
            ("//a//b", "//a/b"),
            ("///a", "/a"),
            ("/..", "/"),
            ("a/../..", ".."),
            ("", "."),
        ] {
            assert_eq!(normpath(p), want, "{p}");
        }
    }

    #[test]
    fn a_spec_with_a_line() {
        assert_eq!(spec_line("a:5"), Some(("a".into(), "5".into(), None)));
        assert_eq!(
            spec_line("a:b:07-9"),
            Some(("a:b".into(), "7".into(), Some("9".into())))
        );
        assert_eq!(spec_line("a:5\n"), Some(("a".into(), "5".into(), None)));
        assert_eq!(spec_line(":5"), None);
        assert_eq!(spec_line("a:5-"), None);
        assert_eq!(spec_line("a\nb:5"), None);
    }

    #[test]
    fn users_from_to() {
        assert_eq!(to_users(" a ,,b,\nc"), vec!["a", "b"]);
        assert!(to_users("").is_empty());
    }

    #[test]
    fn the_words_stop_where_the_shell_s_do() {
        assert!(parse_comment(&["x"]).is_err());
        assert_eq!(
            parse_comment(&["", "x", "t"]).unwrap(),
            Comment {
                id: "x".into(),
                text: "t".into(),
                ..Comment::default()
            }
        );
        assert!(parse_resolve(&["x", "--answered", " \t"]).is_err());
        assert!(parse_resolve(&["x", "--file", "f"]).is_err());
        let a = parse_assign(&["x", "", "--to", "b"]).unwrap();
        assert_eq!(a.users, vec!["", "b"]);
        assert!(parse_assign(&["x"]).is_err());
        assert!(parse_assign(&["x", "--clear"]).unwrap().users.is_empty());
        assert!(parse_settings(&["--brief", "on"]).is_err());
        assert!(parse_settings(&["x"]).is_err());
    }

    #[test]
    fn the_basename() {
        assert_eq!(basename("/a/b/"), "b");
        assert_eq!(basename("/"), "/");
    }
}
