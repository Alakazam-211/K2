//! `k5 feedback ask <title> ...` (`tickets ask`), as the shell CLI's
//! `cmd_feedback_ask` answers it (`cli/src/feedback.sh`).
//!
//! What runs, in the shell's order, once the gates have passed
//! ([`super::write`]):
//!
//! 1. The words ([`parse`]): each flag and its value, a title, one usage
//!    line (`_feedback_err "usage" …`, JSON on stderr, exit 2) at the
//!    first word the loop can't take.
//! 2. The checks after the loop ([`Ctx::ask`]): a title, the kind, the
//!    priority, `--body` with `--body-file`, the body file readable, the
//!    brief (`--html -` runs the shell's `mktemp -t k2-brief-XXXXXX` and
//!    reads stdin; a file must be readable), `--timeout` only with
//!    `--wait` and a whole number, at most 10 files, then the files as
//!    `_thread_attachments_json` writes them ([`super::write::attachments`]),
//!    then `--options`' one note (a comma and no `;`, [`options_one_note`]).
//! 3. `_feedback_detect_session` ([`detect_session`]); with
//!    `TEST_FEEDBACK_DETECT=1` only its kind is printed.
//! 4. The shell's Python, a `python3 -` block with its own urllib client
//!    ([`crate::transport::urllib`], K2's ruling D4): the payload, one
//!    POST of `/cli/feedback/create` with the shell's `$TOKEN` as Bearer,
//!    then the ticket's line, or with `--wait` a GET of
//!    `/cli/feedback/show` every 2 s until it is answered, closed or the
//!    time is up.
//!
//! The answer is read from the response body, never from the
//! environment. Where the shell's Python raises, this prints a traceback
//! whose last line is Python 3.12's; its frames are the block's own
//! (`<stdin>`) and `_ticket_files_check_py`'s (`<string>`), without
//! Python's own modules' frames.

use super::write::{attachments, PyOut};
use crate::families::thread::cards::{option_lines, options_one_note};
use super::{at, at_or, dumps, err_line, join, obj, s, short_text, str_of, PyStr};
use crate::exit::Exit;
use crate::families::{get, py_iter, py_space, Raise, Shown};
use crate::out::json::{self, Text, Value};
use crate::out::py;
use crate::transport::urllib::{decode, Method, Opener, Outcome, Request};
use k2_client::cli_name;
use std::io::Read as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The routes.
pub const CREATE: &str = "/cli/feedback/create";
pub const SHOW: &str = "/cli/feedback/show";
/// `urlopen(..., timeout=30)`.
const TIMEOUT: Duration = Duration::from_secs(30);
/// The `mktemp` template `--html -` uses (`mktemp -t`).
pub const BRIEF_TEMP: &str = "k2-brief-XXXXXX";

/// The lines of the Python block that raise (`<stdin>`).
pub mod line {
    pub const DIE_CODE: u32 = 20;
    pub const WARN_GET: u32 = 129;
    pub const WARN_W: u32 = 131;
    pub const ERROR: u32 = 123;
    pub const DIE_ERROR: u32 = 124;
    pub const FID: u32 = 136;
    pub const SHORT: u32 = 142;
    pub const JOIN: u32 = 144;
    pub const FILES: u32 = 151;
    pub const REVERSED: u32 = 175;
    pub const ATTS: u32 = 179;
    pub const ABSPATH: u32 = 183;
    pub const NOTE: u32 = 189;
    pub const HUMAN: u32 = 196;
    pub const HUMAN_BODY: u32 = 199;
    pub const HINT: u32 = 204;
}

/// What the words ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Words {
    pub title: String,
    pub body: String,
    pub body_file: String,
    pub html: String,
    pub kind: String,
    /// `--options`, one string split on `;` ([`options`]).
    pub options: String,
    /// `options_set`: `--options` was given (even empty).
    pub options_set: bool,
    /// `--option`, repeated: each label whole.
    pub option: Vec<String>,
    pub priority: String,
    pub assign: String,
    pub wait: bool,
    pub timeout: String,
    pub json: bool,
    pub files: Vec<String>,
}

impl Words {
    /// `--html -`: the brief is read from stdin.
    pub fn reads_stdin(&self) -> bool {
        self.html == "-"
    }
}

const ASK_USAGE: &str = concat!(
    "usage: ",
    cli_name!(),
    " tickets ask <title> [--html f|-] [--body|--body-file f] [--kind k] [--option \"\u{2026}\"]\u{2026} [--options \"a; b\"] [--priority 1-5] [--assign user[,user\u{2026}]] [--file path[:line]]\u{2026} [--wait [--timeout s]] [--json]"
);

const MIX: &str = "mix of --options and --option is not allowed";

fn usage(hint: &str) -> String {
    err_line("usage", hint)
}

/// The words after `ask`: what they ask for, or the usage line that stops
/// the loop (exit 2).
pub fn parse(rest: &[&str]) -> Result<Words, String> {
    let mut w = Words::default();
    let mut i = 0;
    while let Some(&a) = rest.get(i) {
        let value = |hint: &str| -> Result<String, String> {
            rest.get(i + 1)
                .map(|v| (*v).to_string())
                .ok_or_else(|| usage(hint))
        };
        match a {
            "--file" => w
                .files
                .push(value("--file requires a path (path[:line[-end]])")?),
            "--html" => {
                w.html = value(concat!(
                    "--html requires a file path (or - for stdin). Start from `",
                    cli_name!(),
                    " tickets template`."
                ))?
            }
            "--body" => w.body = value("--body requires a value")?,
            "--body-file" => w.body_file = value("--body-file requires a file path")?,
            "--kind" => w.kind = value("--kind requires a value (question | approval | fyi)")?,
            "--options" => {
                w.options = value(
                    "--options requires a list with ';' between options (e.g. \"Ship it; Hold\")",
                )?;
                if !w.option.is_empty() {
                    return Err(usage(MIX));
                }
                w.options_set = true;
            }
            "--option" => {
                let label = value("--option requires a label")?;
                if w.options_set {
                    return Err(usage(MIX));
                }
                w.option.push(label);
            }
            "--priority" => w.priority = value("--priority requires a value (1-5)")?,
            "--assign" | "--to" => {
                w.assign =
                    value("--assign requires a comma-separated username list (e.g. owner,julie)")?
            }
            "--wait" => {
                w.wait = true;
                i += 1;
                continue;
            }
            "--timeout" => w.timeout = value("--timeout requires a value in seconds")?,
            "--json" => {
                w.json = true;
                i += 1;
                continue;
            }
            f if f.starts_with('-') => {
                return Err(usage(&format!("unknown flag '{f}' \u{2014} {ASK_USAGE}")))
            }
            t => {
                if w.title.is_empty() {
                    w.title = t.to_string();
                } else {
                    return Err(usage(&format!(
                        "unexpected extra argument '{t}' \u{2014} quote the title if it contains spaces"
                    )));
                }
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    Ok(w)
}

/// `_feedback_detect_session`: the session's id and kind from `lookup`
/// (each setting under its K5_, K2_ and K2SO_ names, empty counts as
/// unset), first match wins.
pub fn detect_session(lookup: impl Fn(&str) -> Option<String>) -> (String, String) {
    use k2_client::env_names::{first_nonempty_in, API_CELL, CELL, SESSION_ID};
    let first = |names: &[&'static str]| first_nonempty_in(names, &lookup).map(|(_, v)| v);
    let id = first(SESSION_ID).unwrap_or_default();
    let cell = first(CELL).unwrap_or_default();
    if first(API_CELL).as_deref() == Some("1") {
        return (id, "api".into());
    }
    if cell == "sidecar" {
        return (id, "sidecar".into());
    }
    if cell == "canonical" || (!id.is_empty() && cell.is_empty()) {
        return (id, "canonical".into());
    }
    for name in ["CLAUDE_SESSION_ID", "CLAUDE_CODE_SESSION_ID"] {
        if let Some(v) = lookup(name).filter(|v| !v.is_empty()) {
            return (v, "canonical".into());
        }
    }
    (String::new(), String::new())
}

/// Everything `ask` reads besides its words.
pub struct Ctx {
    pub opener: Opener,
    /// `$PROJECT`.
    pub project: String,
    /// `_envn AGENT_NAME`.
    pub agent: String,
    /// `_feedback_detect_session`'s id and kind.
    pub session: (String, String),
    /// `_envn TEST_FEEDBACK_DETECT` is `1`.
    pub detect_only: bool,
    /// The shell's `mktemp`, for `--html -`.
    pub mktemp: Option<PathBuf>,
    /// `$PWD` and `HOME`, for the files.
    pub cwd: String,
    pub home: Option<String>,
}

/// `[ -f p ] && [ -r p ]`.
pub fn readable_file(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_file()) && std::fs::File::open(p).is_ok()
}

/// What the checks give the Python block.
struct Checked {
    words: Words,
    /// The brief's bytes when it came from stdin.
    stdin_brief: Option<Vec<u8>>,
    files: Option<Value>,
}

impl Ctx {
    /// `cmd_feedback_ask` after its loop: `words` or the line that stops it.
    pub fn ask(&self, words: Result<Words, String>) -> Exit {
        let words = match words {
            Ok(w) => w,
            Err(l) => return stop(&l, Exit::Usage),
        };
        let checked = match self.check(words) {
            Ok(c) => c,
            Err(e) => return e,
        };
        if checked.words.options_set {
            if let Some(note) = options_one_note(&checked.words.options) {
                crate::out::stderr_line(note);
            }
        }
        if self.detect_only {
            return Shown::out(format!("{}\n", self.session.1)).print();
        }
        self.python(checked)
    }

    fn check(&self, w: Words) -> Result<Checked, Exit> {
        let bad = |hint: String| Err(stop(&usage(&hint), Exit::Usage));
        if w.title.is_empty() {
            return bad("ask requires a <title>".into());
        }
        let kind = if w.kind.is_empty() {
            "question"
        } else {
            &w.kind
        };
        if !matches!(kind, "question" | "approval" | "fyi") {
            return bad(format!(
                "invalid --kind '{}' \u{2014} valid: question, approval, fyi",
                w.kind
            ));
        }
        if !w.priority.is_empty() && !matches!(w.priority.as_str(), "1" | "2" | "3" | "4" | "5") {
            return bad("--priority must be 1-5".into());
        }
        if !w.body.is_empty() && !w.body_file.is_empty() {
            return bad("--body and --body-file are mutually exclusive".into());
        }
        if !w.body_file.is_empty() && !readable_file(&w.body_file) {
            return bad(format!(
                "--body-file: no such readable file '{}'",
                w.body_file
            ));
        }
        let mut stdin_brief = None;
        if w.reads_stdin() {
            stdin_brief = Some(self.brief_from_stdin()?);
        } else if !w.html.is_empty() && !readable_file(&w.html) {
            return bad(format!("--html: no such readable file '{}'", w.html));
        }
        if !w.timeout.is_empty() {
            if !w.wait {
                return bad("--timeout only makes sense with --wait".into());
            }
            if w.timeout == "0" || !w.timeout.bytes().all(|b| b.is_ascii_digit()) {
                return bad("--timeout must be a positive number of seconds".into());
            }
        }
        if w.files.len() > 10 {
            return bad("at most 10 files per message".into());
        }
        let files =
            (!w.files.is_empty()).then(|| attachments(&w.files, &self.cwd, self.home.as_deref()));
        Ok(Checked {
            words: w,
            stdin_brief,
            files,
        })
    }

    /// `html_path="$(mktemp -t k2-brief-XXXXXX)"`, then `cat > "$html_path"`:
    /// the brief, read whole. The file goes again at once (the shell's
    /// EXIT trap removes it); the brief is held here, not read back.
    fn brief_from_stdin(&self) -> Result<Vec<u8>, Exit> {
        let made = self.mktemp.as_ref().and_then(|m| {
            let mut cmd = Command::new(m);
            // The shell runs it by name: its messages say `mktemp`.
            #[cfg(unix)]
            std::os::unix::process::CommandExt::arg0(&mut cmd, "mktemp");
            cmd.args(["-t", BRIEF_TEMP])
                .stdin(Stdio::null())
                .stderr(Stdio::inherit())
                .output()
                .ok()
        });
        let path = match made {
            Some(o) if o.status.success() => {
                let mut p = o.stdout;
                while p.last() == Some(&b'\n') {
                    p.pop();
                }
                path_of(p)
            }
            _ => {
                return Err(stop(
                    &err_line("error", "--html -: cannot create a temp file"),
                    Exit::Error,
                ))
            }
        };
        let mut brief = Vec::new();
        let read = std::io::stdin().lock().read_to_end(&mut brief);
        let _ = std::fs::remove_file(&path);
        if read.is_err() {
            return Err(stop(&usage("--html -: cannot read stdin"), Exit::Usage));
        }
        Ok(brief)
    }

    /// The Python block.
    fn python(&self, c: Checked) -> Exit {
        let mut o = PyOut::default();
        match self.run_python(c, &mut o) {
            Ok(exit) => o.done(exit),
            Err(r) => o.raised(&r),
        }
        .print()
    }

    fn run_python(&self, c: Checked, o: &mut PyOut) -> Result<Exit, Raise> {
        let w = &c.words;
        let mut payload: Vec<(&str, Value)> =
            vec![("project", s(&self.project)), ("title", s(&w.title))];
        if !w.body_file.is_empty() {
            let raw = match std::fs::read(&w.body_file) {
                Ok(b) => b,
                Err(e) => {
                    return die_with(
                        o,
                        "usage",
                        &format!(
                            "--body-file: cannot read '{}': {}",
                            w.body_file,
                            strerror(&e)
                        ),
                    )
                }
            };
            let Ok(text) = String::from_utf8(raw) else {
                return die_with(
                    o,
                    "usage",
                    &format!("--body-file: '{}' is not UTF-8 text", w.body_file),
                );
            };
            let text = text.trim_end_matches('\n');
            if !text.is_empty() {
                payload.push(("body", s(text)));
            }
        } else if !w.body.is_empty() {
            payload.push(("body", s(&w.body)));
        }
        let mut sent_brief = false;
        if !w.html.is_empty() {
            let raw = match &c.stdin_brief {
                Some(b) => b.clone(),
                None => match std::fs::read(&w.html) {
                    Ok(b) => b,
                    Err(e) => {
                        return die_with(
                            o,
                            "usage",
                            &format!("--html: cannot read '{}': {}", w.html, strerror(&e)),
                        )
                    }
                },
            };
            let Ok(text) = String::from_utf8(raw) else {
                return die_with(
                    o,
                    "brief_invalid",
                    concat!(
                        "brief_invalid: the --html file is not UTF-8. Save it as UTF-8 and ask again. See `",
                        cli_name!(),
                        " study ticket-brief`."
                    ),
                );
            };
            payload.push(("briefHtml", s(&text)));
            sent_brief = true;
        }
        if !w.kind.is_empty() {
            payload.push(("kind", s(&w.kind)));
        }
        if !w.priority.is_empty() {
            payload.push(("priority", Value::Int(w.priority.clone())));
        }
        let opts = if !w.options.is_empty() {
            options(&w.options)
        } else if !w.option.is_empty() {
            option_list(&w.option)
        } else {
            Vec::new()
        };
        if !opts.is_empty() {
            payload.push(("options", Value::Array(opts.iter().map(|x| s(x)).collect())));
        }
        if !w.assign.is_empty() {
            let names: Vec<Value> = w
                .assign
                .split(',')
                .map(|p| p.trim_matches(py_space))
                .filter(|p| !p.is_empty())
                .map(s)
                .collect();
            if !names.is_empty() {
                payload.push(("assignees", Value::Array(names)));
            }
        }
        if !self.agent.is_empty() {
            payload.push(("agentName", s(&self.agent)));
        }
        let mut sent_files = false;
        if let Some(files) = &c.files {
            sent_files = py::truthy(files);
            payload.push(("attachments", files.clone()));
        }
        if !self.session.0.is_empty() {
            payload.push(("sessionId", s(&self.session.0)));
            if !self.session.1.is_empty() {
                payload.push(("sessionKind", s(&self.session.1)));
            }
        }
        let body = dumps(&obj(payload));
        let Some(d) = self.request(Method::Post, CREATE, None, Some(body.into_bytes())) else {
            return die_with(
                o,
                "daemon_unreachable",
                concat!(
                    "no response from the ",
                    k2_client::product_name!(),
                    " daemon (is it running?)"
                ),
            );
        };
        let Value::Object(pairs) = &d else {
            return Err(Raise::no_attribute(line::ERROR, &d, "get"));
        };
        if let Some(err) = get(pairs, "error").filter(|e| py::truthy(e)) {
            return die(o, err, line::DIE_ERROR);
        }
        if !at(&d, "ok").is_some_and(py::truthy) {
            let shown: String = dumps(&d).chars().take(200).collect();
            let hint = format!(
                "unexpected daemon response: {}",
                py::repr(&Value::Str(Text::from(shown)))
            );
            return die_with(o, "error", &hint);
        }
        if let Some(e) = super::write::ticket_files_check(sent_files, &d) {
            o.stderr.push_str(&e);
            return Ok(Exit::Error);
        }
        let warnings = at(&d, "warnings")
            .filter(|v| py::truthy(v))
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        for w in py_iter(&warnings, line::WARN_GET)? {
            if let Value::Object(_) = w {
                let code = at(&w, "code").filter(|v| py::truthy(v)).cloned();
                let hint = at(&w, "hint").filter(|v| py::truthy(v)).cloned();
                warn(
                    o,
                    &code.map_or_else(|| "warning".into(), |v| str_of(&v)),
                    &hint.map_or_else(String::new, |v| str_of(&v)),
                );
            }
        }
        if sent_brief && at(&d, "hasBrief") != Some(&Value::Bool(true)) {
            warn(
                o,
                "brief_not_stored",
                "this server is older than 0.43.2 and dropped the HTML brief",
            );
        }
        let Some(fid) = at(&d, "id").cloned() else {
            return Err(Raise::at(line::FID, "KeyError: 'id'"));
        };
        if !w.wait {
            if w.json {
                o.line(&dumps(&d));
                return Ok(Exit::Ok);
            }
            let short = slice8(&fid, line::SHORT)?;
            let assignees = at(&d, "assignees")
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let mut who = if py::truthy(&assignees) {
                format!(
                    " \u{2192} {}",
                    join(&assignees, ", ").map_err(|e| Raise::at(line::JOIN, e))?
                )
            } else {
                String::new()
            };
            if at(&d, "defaultAssignee").is_some_and(py::truthy) {
                who.push_str(" (this server's default assignee)");
            }
            o.line(&format!(
                "Filed feedback {short} \u{2014} \"{}\" ({}, priority {}, {}){who}",
                str_of(&at_or(&d, "title", s(""))),
                str_of(&at_or(&d, "kind", s("?"))),
                str_of(&at_or(&d, "priority", s("?"))),
                str_of(&at_or(&d, "status", s("?"))),
            ));
            let atts = at_or(&d, "attachments", Value::Null);
            for l in super::ticket_file_lines_from(&atts, &Value::Null, "  ", line::FILES)? {
                o.line(&l);
            }
            o.line(&format!(
                "A human will be notified. Check back with: {} tickets show {short}",
                k2_client::brand::CLI_NAME
            ));
            return Ok(Exit::Ok);
        }
        self.wait(&fid, &w.timeout, w.json, o)
    }

    /// `--wait`: the GET of `show` every 2 s until it ends.
    fn wait(
        &self,
        fid: &Value,
        timeout: &str,
        as_json: bool,
        o: &mut PyOut,
    ) -> Result<Exit, Raise> {
        let deadline =
            (!timeout.is_empty()).then(|| now() + timeout.parse::<f64>().unwrap_or(f64::INFINITY));
        let id = str_of(fid);
        loop {
            let d = self.request(Method::Get, SHOW, Some(&id), None);
            if let Some(d @ Value::Object(_)) = &d {
                if at(d, "ok").is_some_and(py::truthy) {
                    if let Some(exit) = self.settled(d, fid, as_json, o)? {
                        return Ok(exit);
                    }
                }
            }
            if deadline.is_some_and(|t| now() >= t) {
                o.stderr.push_str("{\"error\":{\"code\":\"timeout\"}}\n");
                return Ok(Exit::Error);
            }
            let mut nap = 2.0f64;
            if let Some(t) = deadline {
                nap = (t - now()).min(nap).max(0.1);
            }
            std::thread::sleep(Duration::from_secs_f64(nap));
        }
    }

    /// One `show` answer that said `ok`: the exit when it ends the wait.
    fn settled(
        &self,
        d: &Value,
        fid: &Value,
        as_json: bool,
        o: &mut PyOut,
    ) -> Result<Option<Exit>, Raise> {
        let status = at_or(d, "status", Value::Null);
        let answer = at_or(d, "answer", Value::Null);
        let is = |word: &str| status == s(word);
        if is("answered") && py::truthy(&answer) {
            let comments = at(d, "comments")
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let mut list = match &comments {
                Value::Array(_) | Value::Str(_) => py_iter(&comments, line::REVERSED)?,
                other => {
                    return Err(Raise::at(
                        line::REVERSED,
                        format!(
                            "TypeError: '{}' object is not reversible",
                            py::type_name(other)
                        ),
                    ))
                }
            };
            list.reverse();
            let want = strip(&PyStr::of(&answer));
            let msg = list.into_iter().find(|c| {
                matches!(c, Value::Object(_)) && strip(&PyStr::of(&at_or(c, "body", s("")))) == want
            });
            let from = msg
                .as_ref()
                .and_then(|m| at(m, "attachments"))
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let mut atts: Vec<Value> = py_iter(&from, line::ATTS)?
                .into_iter()
                .filter(|a| matches!(a, Value::Object(_)))
                .collect();
            let root = at_or(d, "projectPath", Value::Null);
            for a in &mut atts {
                let path = at_or(a, "path", Value::Null);
                if py::truthy(&root) && py::truthy(&path) {
                    let joined =
                        path_join(&root, &path).map_err(|e| Raise::at(line::ABSPATH, e))?;
                    set_key(a, "absPath", s(&joined));
                }
            }
            if as_json {
                o.line(&dumps(&obj(vec![
                    ("ok", Value::Bool(true)),
                    ("id", fid.clone()),
                    ("status", status.clone()),
                    ("answer", answer.clone()),
                    ("attachments", Value::Array(atts)),
                ])));
                return Ok(Some(Exit::Ok));
            }
            if !atts.is_empty() {
                let short = slice8(fid, line::NOTE)?;
                o.stderr.push_str(&format!(
                    "{} file{} attached: {} tickets show {short}\n",
                    atts.len(),
                    if atts.len() == 1 { "" } else { "s" },
                    k2_client::brand::CLI_NAME
                ));
            }
            o.line(&str_of(&answer));
            return Ok(Some(Exit::Ok));
        }
        if is("needs_discussion") {
            let comments = at(d, "comments")
                .filter(|v| py::truthy(v))
                .cloned()
                .unwrap_or(Value::Array(Vec::new()));
            let agent = at_or(d, "agentName", Value::Null);
            let human: Vec<Value> = py_iter(&comments, line::HUMAN)?
                .into_iter()
                .filter(|c| {
                    matches!(c, Value::Object(_))
                        && !py_eq(&at_or(c, "author", Value::Null), &agent)
                })
                .collect();
            if let Some(last) = human.last() {
                o.line(&str_of(&at_or(last, "body", s(""))));
            }
            let short = slice8(fid, line::HINT)?;
            let hint = format!(
                "The person wrote a message instead of picking an option. Reply with: {cli} tickets comment {short} \"<reply>\". When settled: {cli} tickets resolve {short} --answered \"<outcome>\" (or resolve).",
                cli = k2_client::brand::CLI_NAME
            );
            o.stderr.push_str(&dumps(&obj(vec![
                ("status", s("needs_discussion")),
                ("hint", s(&hint)),
            ])));
            o.stderr.push('\n');
            return Ok(Some(Exit::Conflict));
        }
        if is("resolved") || is("dismissed") {
            if py::truthy(&answer) {
                o.line(&str_of(&answer));
                return Ok(Some(Exit::Ok));
            }
            o.stderr
                .push_str("{\"error\":{\"code\":\"closed_unanswered\"}}\n");
            return Ok(Some(Exit::Error));
        }
        Ok(None)
    }

    /// The block's `request(method, route, params, body)`: the answer as
    /// JSON (an HTTP error's body too, or its `HTTP <code> on <route>`
    /// error), or `None` when anything else failed.
    fn request(
        &self,
        method: Method,
        route: &str,
        id: Option<&str>,
        data: Option<Vec<u8>>,
    ) -> Option<Value> {
        let mut req = Request::new(method, route, TIMEOUT);
        if let Some(id) = id {
            req = req.query([("id", id)]);
        }
        if let Some(d) = data {
            req = req.data(d).header("Content-Type", "application/json");
        }
        if let Some(t) = self.opener.token() {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        match self.opener.open(&req) {
            Outcome::Answer { body: Ok(b), .. } => json::parse(&decode(&b)).ok(),
            Outcome::HttpError { code, body } => {
                let parsed = body.ok().and_then(|b| json::parse(&decode(&b)).ok());
                Some(parsed.unwrap_or_else(|| {
                    obj(vec![(
                        "error",
                        obj(vec![
                            ("code", s("error")),
                            ("hint", s(&format!("HTTP {code} on {route}"))),
                        ]),
                    )])
                }))
            }
            Outcome::Answer { body: Err(_), .. } | Outcome::Raised(_) => None,
        }
    }
}

/// Prints `line` on stderr; `exit`.
fn stop(line: &str, exit: Exit) -> Exit {
    crate::out::stderr_line(line);
    exit
}

/// `time.time()`.
fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

fn path_of(bytes: Vec<u8>) -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(bytes))
    }
    #[cfg(not(unix))]
    {
        PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// `e.strerror`.
fn strerror(e: &std::io::Error) -> String {
    let full = e.to_string();
    match e.raw_os_error() {
        Some(n) => full
            .strip_suffix(&format!(" (os error {n})"))
            .unwrap_or(&full)
            .to_string(),
        None => full,
    }
}

/// `--options`: split on `;` only, each stripped, the empty ones left
/// out. A comma is part of a label.
pub fn options(raw: &str) -> Vec<String> {
    raw.split(';')
        .map(|p| p.trim_matches(py_space))
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// `--option`, repeated: the labels as `printf '%s\n'` hands them to
/// Python's `json.dumps` line by line, then each stripped, the empty ones
/// left out.
pub fn option_list(labels: &[String]) -> Vec<String> {
    option_lines(labels)
        .iter()
        .map(|p| p.trim_matches(py_space))
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// `warn(code, hint)`: one line of JSON on stderr.
fn warn(o: &mut PyOut, code: &str, hint: &str) {
    o.stderr.push_str(&dumps(&obj(vec![(
        "warning",
        obj(vec![("code", s(code)), ("hint", s(hint))]),
    )])));
    o.stderr.push('\n');
}

fn die_with(o: &mut PyOut, code: &str, hint: &str) -> Result<Exit, Raise> {
    die(o, &obj(vec![("code", s(code)), ("hint", s(hint))]), 0)
}

/// The block's `die(err)`: the error as JSON on stderr, then 4 for
/// `not_found` and `ambiguous_id`, 2 for `usage` and the `brief_` and
/// `assignee_` codes, else 1. `err.get` on what is not a dict raises
/// (called from line `from`).
fn die(o: &mut PyOut, err: &Value, from: u32) -> Result<Exit, Raise> {
    let err = match err {
        Value::Str(_) => obj(vec![("code", s("error")), ("hint", err.clone())]),
        other => other.clone(),
    };
    o.stderr
        .push_str(&dumps(&obj(vec![("error", err.clone())])));
    o.stderr.push('\n');
    let Value::Object(pairs) = &err else {
        return Err(Raise::inside(
            from,
            line::DIE_CODE,
            "die",
            Raise::no_attribute(0, &err, "get").exception,
        ));
    };
    let code = get(pairs, "code")
        .filter(|v| py::truthy(v))
        .map_or_else(String::new, str_of);
    Ok(if code == "not_found" || code == "ambiguous_id" {
        Exit::NotFound
    } else if code == "usage" || code.starts_with("brief_") || code.starts_with("assignee_") {
        Exit::Usage
    } else {
        Exit::Error
    })
}

/// `v[:8]`, written as `%s` writes it.
fn slice8(v: &Value, line: u32) -> Result<String, Raise> {
    match v {
        Value::Str(_) => Ok(short_text(v)),
        Value::Array(items) => Ok(py::repr(&Value::Array(
            items.iter().take(8).cloned().collect(),
        ))),
        Value::Object(_) => Err(Raise::at(line, "KeyError: slice(None, 8, None)")),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not subscriptable",
                py::type_name(other)
            ),
        )),
    }
}

/// `s.strip()`.
fn strip(p: &PyStr) -> Vec<u32> {
    let space = |cp: &u32| char::from_u32(*cp).is_some_and(py_space);
    let start = p.0.iter().position(|c| !space(c)).unwrap_or(p.0.len());
    let end = p.0.iter().rposition(|c| !space(c)).map_or(start, |e| e + 1);
    p.0[start..end.max(start)].to_vec()
}

/// A number's value, as Python compares numbers (`True == 1 == 1.0`).
fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Int(d) => d.parse().ok(),
        Value::Float(f) => Some(*f),
        _ => None,
    }
}

/// `a == b` for two JSON values, as Python compares them.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    if let (Some(x), Some(y)) = (number(a), number(b)) {
        if let (Value::Int(p), Value::Int(q)) = (a, b) {
            return p == q;
        }
        return x == y;
    }
    match (a, b) {
        (Value::Null, Value::Null) => true,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter().all(|(k, v)| {
                    get(y, &PyStr(k.code_points()).render()).is_some_and(|w| py_eq(v, w))
                })
        }
        _ => false,
    }
}

/// `os.path.join(root, path)`, or the `TypeError`.
fn path_join(root: &Value, path: &Value) -> Result<String, String> {
    match (root, path) {
        (Value::Str(_), Value::Str(_)) => {
            let (r, p) = (str_of(root), str_of(path));
            Ok(if p.starts_with('/') {
                p
            } else if r.is_empty() || r.ends_with('/') {
                format!("{r}{p}")
            } else {
                format!("{r}/{p}")
            })
        }
        (Value::Str(_), other) | (other, _) => Err(format!(
            "TypeError: expected str, bytes or os.PathLike object, not {}",
            py::type_name(other)
        )),
    }
}

/// `a[key] = v`: the key's place kept when it is there, else last.
fn set_key(a: &mut Value, key: &str, v: Value) {
    if let Value::Object(pairs) = a {
        let k = Text::from(key);
        match pairs.iter_mut().find(|(n, _)| *n == k) {
            Some((_, old)) => *old = v,
            None => pairs.push((k, v)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words() {
        let w = parse(&["t", "--wait", "--timeout", "5", "--to", "a,b", "--json"]).unwrap();
        assert_eq!(
            (
                w.title.as_str(),
                w.wait,
                w.timeout.as_str(),
                w.assign.as_str(),
                w.json
            ),
            ("t", true, "5", "a,b", true)
        );
        // An empty title is taken again.
        assert_eq!(parse(&["", "x"]).unwrap().title, "x");
        assert!(parse(&["a", "b"])
            .unwrap_err()
            .contains("unexpected extra argument 'b'"));
        assert!(parse(&["--body"])
            .unwrap_err()
            .contains("--body requires a value"));
        assert!(parse(&["-"]).unwrap_err().contains("unknown flag '-'"));
        // A value may start with a dash.
        assert_eq!(parse(&["t", "--body", "-x"]).unwrap().body, "-x");
    }

    #[test]
    fn options_split_on_semicolons_only() {
        assert_eq!(
            options("No, the forward clean-up is enough, Yes, rewrite the history too"),
            vec!["No, the forward clean-up is enough, Yes, rewrite the history too"]
        );
        assert_eq!(options(" a ; b ;; "), vec!["a", "b"]);
        assert_eq!(options("a\\,b"), vec!["a\\,b"]);
        assert!(options(" ; ").is_empty());
        assert_eq!(
            option_list(&["No, wait".into(), " ".into(), "a\rb".into()]),
            vec!["No, wait", "a", "b"]
        );
    }

    #[test]
    fn the_session_detected() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |n: &str| {
                pairs
                    .iter()
                    .find(|(k, _)| *k == n)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            detect_session(env(&[("K2_API_CELL", "1"), ("K2_SESSION_ID", "s")])),
            ("s".into(), "api".into())
        );
        assert_eq!(
            detect_session(env(&[("K2_CELL", "sidecar")])),
            ("".into(), "sidecar".into())
        );
        assert_eq!(
            detect_session(env(&[("K2_SESSION_ID", "s")])),
            ("s".into(), "canonical".into())
        );
        assert_eq!(
            detect_session(env(&[
                ("K2_SESSION_ID", "s"),
                ("K2_CELL", "x"),
                ("CLAUDE_CODE_SESSION_ID", "c")
            ])),
            ("c".into(), "canonical".into())
        );
        assert_eq!(detect_session(env(&[])), ("".into(), "".into()));
    }

    #[test]
    fn python_s_equality() {
        assert!(py_eq(&Value::Int("1".into()), &Value::Bool(true)));
        assert!(py_eq(&Value::Float(1.0), &Value::Int("1".into())));
        assert!(!py_eq(&s("1"), &Value::Int("1".into())));
        assert!(py_eq(&Value::Null, &Value::Null));
    }
}
