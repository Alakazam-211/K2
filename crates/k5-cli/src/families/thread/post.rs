//! `k5 thread <addr> "text"` (a write) and `k5 thread <addr>` (a read), as
//! `cmd_thread` does them past its verbs (`cli/src/browser.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The words ([`parse`]): `--json`, `--idempotency-key`, `--from`,
//!    `--limit`, `--all`, `--before-seq`, `--seq`, `--file` (each stop in
//!    the loop's order); the first other word is the address, the last
//!    the text. No address is the usage block, exit 1. `--seq N` must be
//!    a whole number of at least 1 with none of the text, `--file`,
//!    `--limit`, `--all` or `--before-seq`; it reads the newest one
//!    below N+1.
//! 2. The sender ([`sender`]): `--from`, a sidecar cell's
//!    `<primary>/<sidecar>`, else the folder's handle
//!    (`/cli/workspace/handle` over TCP, its refusal no refusal of the
//!    command and its errors hidden), else the command's name (`k2`,
//!    [`CLI_NAME`]). A lookup with no answer
//!    ends the command by SIGPIPE; any other failure is that name (the lookup
//!    runs inside `$(…)`, where `set -e` is off).
//! 3. More than 10 files: exit 1.
//! 4. A write (text or files): the send key
//!    ([`crate::families::msg::send_once::init`]), the form to
//!    `/cli/thread/post` sent once by its key, with the files as
//!    `attachments` ([`attachments`]); the rename and replay notes; the
//!    answer as it came with `--json`, else [`posted`]'s lines; after a
//!    write that printed, the markdown hint ([`md_nudge`]).
//! 5. A read: `cli_send cli_request /cli/thread`, the answer as it came
//!    with `--json` (not `--seq`), else [`shown`]'s lines.
//!
//! A value flag (`--from`, `--limit`, `--before-seq`) as the last word is
//! [`needs_value`]'s line and exit 2, the shell's own (`_need_value`).
//!
//! A `--file` path that starts with `~` expands as Python's
//! `os.path.expanduser` does: `HOME` (with none, this user's folder in the
//! password database), or `~name`'s folder there. The shell answers a
//! `--file` the binary can't resolve as Python's `os.path` would: a line
//! break in it, a digit outside ASCII (Python's `\d`), or a `~` path whose
//! folder is not UTF-8 (or with no password database to read).

use super::super::msg::send::{echo, handle_of, moved_note};
use super::super::msg::send_once::{self, held};
use super::super::{end_in_pipe, without_nuls, Raise};
use super::py::{self, dumps, get_at, get_or, items, line_spec, loads, not_ok, or, s, size};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::py::{str_of, truthy};
use crate::out::{self, pylib};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{Call, Channel, Client, Error, Response};
use k2_client::brand::CLI_NAME;
use k2_client::env_names;

/// The write's route.
pub const POST: &str = "/cli/thread/post";
/// The read's route.
pub const READ: &str = "/cli/thread";
/// The sender's handle.
pub const HANDLE: &str = "/cli/workspace/handle";
/// The most files one message takes.
pub const MAX_FILES: usize = 10;

/// What the words say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub target: String,
    pub text: String,
    pub json: bool,
    pub from: String,
    pub limit: String,
    pub all: bool,
    pub before_seq: String,
    /// `--seq`'s number, normalised (`$((10#N))`); empty for none.
    pub one_seq: String,
    pub idem_key: String,
    pub files: Vec<String>,
}

/// The words, or the lines and exit that stop the command first.
pub type Parsed = Result<Line, (String, Exit)>;

/// The usage block for no address.
pub fn usage() -> String {
    let c = CLI_NAME;
    format!(
        "Error: address required\n\
         Usage: {c} thread <addr> \"text\"     # write overlay text\n       \
         {c} thread <addr> [\"text\"] --file path[:line[-end]]   # attach files\n       \
         {c} thread <addr> [--json] [--limit N|--all] [--before-seq N]\n       \
         {c} thread ask <addr> \"prompt\" --option \"…\" --option \"…\"   (or --options \"a; b; c\")\n       \
         {c} thread secret <addr> --name NAME\n       \
         {c} thread withdraw <addr> <card-id> [--why \"…\"]\n       \
         {c} thread dismiss <addr> <card-id> [--why \"…\"]\n"
    )
}

/// `-*)`.
pub fn unknown_flag(word: &str) -> String {
    format!("Error: unknown flag '{word}' (try '{CLI_NAME} thread --help')\n")
}

/// A value flag as the last word: the shell's `_need_value` line
/// ([`crate::out::need_value`]), exit 2.
pub fn needs_value(flag: &str) -> String {
    format!("{}\n", crate::out::need_value(flag))
}

/// The idempotency key's line.
pub const IDEM_USAGE: &str =
    "Error: --idempotency-key needs the key a 'may have been delivered' line printed\n";

fn seq_needs() -> String {
    format!("Error: --seq needs a message number (the [N] in {CLI_NAME} thread <addr>)\n")
}

fn seq_whole() -> String {
    format!("Error: --seq needs a whole number >= 1 (the [N] in {CLI_NAME} thread <addr>)\n")
}

/// `--seq` with anything else.
pub const SEQ_ALONE: &str =
    "Error: --seq reads one message: no text, --file, --limit, --all or --before-seq with it\n";
/// `--file` with no path.
pub const FILE_NEEDS: &str = "Error: --file needs a path (path[:line[-end]])\n";
/// Too many files.
pub const TOO_MANY_FILES: &str = "Error: at most 10 files per message";

/// The words after `thread`, as `cmd_thread`'s loop reads them. `None`:
/// the shell answers (a `--file` the binary can't resolve).
pub fn parse(words: &[&str]) -> Option<Parsed> {
    let mut l = Line::default();
    let stop = |t: String, e: Exit| Some(Err((t, e)));
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        let next = words.get(i + 1).copied();
        match w {
            "--json" => {
                l.json = true;
                i += 1;
            }
            "--all" => {
                l.all = true;
                i += 1;
            }
            "--idempotency-key" => match next {
                Some(k) if !k.is_empty() => {
                    l.idem_key = k.to_string();
                    i += 2;
                }
                _ => return stop(IDEM_USAGE.into(), Exit::Usage),
            },
            "--from" | "--limit" | "--before-seq" => {
                let Some(v) = next else {
                    return stop(needs_value(w), Exit::Usage);
                };
                *match w {
                    "--from" => &mut l.from,
                    "--limit" => &mut l.limit,
                    _ => &mut l.before_seq,
                } = v.to_string();
                i += 2;
            }
            "--seq" => match next {
                Some(v) if !v.is_empty() => {
                    l.one_seq = v.to_string();
                    i += 2;
                }
                _ => return stop(seq_needs(), Exit::Error),
            },
            "--file" => match next {
                Some(v) if !v.is_empty() => {
                    if !resolvable(v) {
                        return None;
                    }
                    l.files.push(v.to_string());
                    i += 2;
                }
                _ => return stop(FILE_NEEDS.into(), Exit::Error),
            },
            // The rest is the address and the text, as typed.
            "--" => {
                for w in &words[i + 1..] {
                    if l.target.is_empty() {
                        l.target = w.to_string();
                    } else {
                        l.text = w.to_string();
                    }
                }
                i = words.len();
            }
            w if w.starts_with('-') => return stop(unknown_flag(w), Exit::Error),
            w => {
                if l.target.is_empty() {
                    l.target = w.to_string();
                } else {
                    l.text = w.to_string();
                }
                i += 1;
            }
        }
    }
    if l.target.is_empty() {
        return stop(usage(), Exit::Error);
    }
    if !l.one_seq.is_empty() {
        let n = match bash_number(&l.one_seq) {
            Some(n) if n >= 1 => n,
            _ => return stop(seq_whole(), Exit::Error),
        };
        if !l.text.is_empty()
            || !l.files.is_empty()
            || !l.limit.is_empty()
            || l.all
            || !l.before_seq.is_empty()
        {
            return stop(SEQ_ALONE.into(), Exit::Error);
        }
        l.one_seq = n.to_string();
        l.before_seq = n.wrapping_add(1).to_string();
        l.limit = "1".into();
    }
    Some(Ok(l))
}

/// `[[ "$n" =~ ^[0-9]+$ ]]`, then `$((10#$n))` (64-bit, wrapping).
pub fn bash_number(n: &str) -> Option<i64> {
    if n.is_empty() || !n.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(n.bytes().fold(0i64, |acc, b| {
        acc.wrapping_mul(10).wrapping_add(i64::from(b - b'0'))
    }))
}

/// A `--file` spec [`attachments`] reads as Python does: both ways it
/// may read it, the whole spec and its path before a `:N`.
pub fn resolvable(spec: &str) -> bool {
    if spec.contains(['\n', '\r']) || spec.chars().any(|c| c.is_numeric() && !c.is_ascii_digit()) {
        return false;
    }
    expanduser(spec).is_some()
        && split_line(spec).is_none_or(|(path, _, _)| expanduser(&path).is_some())
}

/// The write or the read (steps 2 to 5 of the module's notes).
pub fn run(client: &mut Client, closing: &mut Closing, parsed: Parsed) -> Exit {
    let l = match parsed {
        Ok(l) => l,
        Err((text, exit)) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
            return exit;
        }
    };
    let from = sender(client, closing, &l.from);
    if l.files.len() > MAX_FILES {
        out::stderr_line(TOO_MANY_FILES);
        return Exit::Error;
    }
    if !l.text.is_empty() || !l.files.is_empty() {
        write(client, closing, &l, &from)
    } else {
        read(client, closing, &l)
    }
}

/// `$PWD` as the shell has it.
pub fn folder() -> String {
    crate::daemon::logical_cwd(std::env::var_os("PWD"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `_thread_sender_from` (step 2 of the module's notes).
pub fn sender(client: &mut Client, closing: &mut Closing, explicit: &str) -> String {
    if !explicit.is_empty() {
        return explicit.to_string();
    }
    let envn = super::super::msg::envn;
    let cell = envn(env_names::CELL).unwrap_or_default();
    let primary = envn(env_names::PRIMARY).unwrap_or_default();
    let side = envn(env_names::SIDECAR_NAME).unwrap_or_default();
    if cell == "sidecar" && !primary.is_empty() && !side.is_empty() {
        return format!("{primary}/{side}");
    }
    let path = envn(env_names::PROJECT_PATH).unwrap_or_else(folder);
    if path.is_empty() {
        return CLI_NAME.into();
    }
    let arg = format!("project={}", quote(&path));
    let passport = client.passport().is_some();
    let name = match client.call(Channel::Tcp, HANDLE, Call::Request(&[arg.as_str()])) {
        Ok(r) => {
            // A lookup's refusal never closes the command.
            closing.answer(
                &Response {
                    permission: None,
                    ..r.clone()
                },
                passport,
            );
            handle_of(&without_nuls(&r.body))
        }
        Err(Error::NoAnswer(_)) => end_in_pipe(),
        Err(_) => String::new(),
    };
    if name.is_empty() {
        CLI_NAME.into()
    } else {
        name
    }
}

/// `_thread_attachments_json "$PWD" <specs>`: each spec as `{path,
/// line?, lineEnd?}`. A `:N` or `:N-M` ending is a line only when the
/// whole spec is no file here; a path that is a file here goes absolute.
pub fn attachments(cwd: &str, specs: &[String]) -> String {
    // [`resolvable`] took every spec, so each expands.
    let expand = |p: &str| expanduser(p).unwrap_or_else(|| p.to_string());
    let here = |p: &str| pylib::path::join(cwd, &expand(p));
    let mut out = Vec::new();
    for spec in specs {
        let (mut path, mut line, mut end) = (spec.clone(), None, None);
        if !pylib::path::exists(&here(spec)) {
            if let Some((p, l, e)) = split_line(spec) {
                path = p;
                line = Some(l);
                end = e;
            }
        }
        if pylib::path::exists(&here(&path)) {
            path = pylib::path::abspath(&here(&path));
        }
        let mut item = vec![(Text::from("path"), s(&path))];
        if let Some(l) = line {
            item.push((Text::from("line"), Value::Int(l)));
        }
        if let Some(e) = end {
            item.push((Text::from("lineEnd"), Value::Int(e)));
        }
        out.push(Value::Object(item));
    }
    dumps(&Value::Array(out))
}

/// `os.path.expanduser(p)` ([`pylib::path::expanduser`]) with this
/// process's `HOME`. `None`: the binary can't tell what Python gives (a
/// `HOME` or a folder that is not UTF-8, no password database).
fn expanduser(p: &str) -> Option<String> {
    if !p.starts_with('~') {
        return Some(p.to_string());
    }
    let home = std::env::var_os("HOME")
        .map(|h| h.into_string())
        .transpose()
        .ok()?;
    pylib::path::expanduser(p, home.as_deref())
}

/// `re.match(r"^(.+?):(\d+)(?:-(\d+))?$", spec)`: the path, the line and
/// the end (as `int()` writes them).
pub fn split_line(spec: &str) -> Option<(String, String, Option<String>)> {
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let int = |t: &str| {
        let n = t.trim_start_matches('0');
        if n.is_empty() {
            "0".to_string()
        } else {
            n.to_string()
        }
    };
    for (i, _) in spec.match_indices(':').filter(|(i, _)| *i > 0) {
        let rest = &spec[i + 1..];
        let (l, e) = match rest.split_once('-') {
            Some((l, e)) => (l, Some(e)),
            None => (rest, None),
        };
        if digits(l) && e.is_none_or(digits) {
            return Some((spec[..i].to_string(), int(l), e.map(int)));
        }
    }
    None
}

/// The form a write sends.
pub fn form(l: &Line, from: &str, key: &str, attachments: Option<&str>) -> String {
    let mut p = format!(
        "addr={}&text={}&from={}&via=thread&clientMsgId={}",
        quote(&l.target),
        quote(&l.text),
        quote(from),
        quote(key)
    );
    if let Some(a) = attachments {
        p.push_str(&format!("&attachments={}", quote(a)));
    }
    p
}

/// The query a read sends.
pub fn query(l: &Line) -> String {
    let mut p = format!("addr={}", quote(&l.target));
    if l.all {
        p.push_str("&limit=0");
    } else if !l.limit.is_empty() {
        p.push_str(&format!("&limit={}", quote(&l.limit)));
    }
    if !l.before_seq.is_empty() {
        p.push_str(&format!("&before_seq={}", quote(&l.before_seq)));
    }
    p
}

/// The rename and replay notes on stderr.
pub fn notes(response: &[u8], replay: bool) {
    let d = loads(response);
    if let Some(n) = d.as_ref().and_then(moved_note) {
        out::stderr_line(&n);
    }
    if replay {
        if let Some(n) = send_once::replay_note(response) {
            out::stderr_line(&n);
        }
    }
}

/// `echo "$response"`.
pub fn echo_out(response: &[u8]) -> Exit {
    match out::write_flushed(&mut std::io::stdout().lock(), &echo(response)) {
        Ok(()) => Exit::Ok,
        Err(e) => out::after_stdout_error(&e),
    }
}

fn write(client: &mut Client, closing: &mut Closing, l: &Line, from: &str) -> Exit {
    let key = match send_once::init(client, &l.idem_key) {
        Ok(k) => k,
        Err(exit) => return exit,
    };
    let att = (!l.files.is_empty()).then(|| attachments(&folder(), &l.files));
    let params = form(l, from, &key.key, att.as_deref());
    closing.reset_note();
    let soft = match send_once::send(
        client,
        closing,
        &key,
        POST,
        Call::PostForm(&[params.as_str()]),
    ) {
        Ok(s) => s,
        Err(exit) => return exit,
    };
    if soft.rc != 0 {
        return Exit::from_curl(soft.rc);
    }
    notes(&soft.out, true);
    if l.json {
        return echo_out(&soft.out);
    }
    let exit = posted(&echo(&soft.out)).print();
    if exit == Exit::Ok {
        md_nudge(client);
    }
    exit
}

/// The write's Python on what `echo` handed it.
pub fn posted(raw: &[u8]) -> py::Shown {
    let mut o = py::Shown::default();
    let Some(d) = loads(raw) else {
        o.stdout = raw.to_vec();
        return o;
    };
    match posted_with(&d, &mut o) {
        Ok(code) => o.exit(code),
        Err(r) => o.raised(&r),
    }
}

fn posted_with(d: &Value, o: &mut py::Shown) -> Result<i32, Raise> {
    if not_ok(d, 8)? {
        let (line, _) = py::error_line(d, 9)?;
        o.err(&line);
        return Ok(2);
    }
    let get = |k: &str| get_or(d, k, s(""), 15);
    o.line(&format!(
        "{}  seq={}  from={}",
        str_of(&get("id")?),
        str_of(&get("seq")?),
        str_of(&get("from")?)
    ));
    for a in items(get_at(d, "attachments", 22)?, 22)? {
        let line = line_spec(&a, 24)?;
        let path = get_or(&a, "path", s(""), 26)?;
        let sz = size(&get_at(&a, "size", 26)?, 26)?;
        o.line(&format!("  [file] {}{line} ({sz})", str_of(&path)));
    }
    Ok(0)
}

/// `_thread_md_nudge`: after a write that printed, the first and every
/// 10th, the markdown hint on stderr (the count in
/// `<workspace>/.k2/.thread-md-nudge`).
pub fn md_nudge(client: &Client) {
    let root = super::super::msg::envn(env_names::PROJECT_PATH)
        .or_else(|| (!client.project.is_empty()).then(|| client.project.clone()))
        .unwrap_or_else(folder);
    let dot = k2_client::workspace_dir::workspace_dot_dir(&root);
    if !dot.is_dir() {
        return;
    }
    let file = dot.join(".thread-md-nudge");
    let mut n: i64 = 0;
    if file.is_file() {
        let held: String = std::fs::read(&file)
            .map(|b| {
                b.iter()
                    .filter(|c| !matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
                    .map(|c| *c as char)
                    .collect()
            })
            .unwrap_or_default();
        if !held.is_empty() && held.bytes().all(|b| b.is_ascii_digit()) {
            match bash_arith(&held) {
                Some(v) => n = v,
                None => return,
            }
        }
    }
    n = n.wrapping_add(1);
    if std::fs::write(&file, format!("{n}\n")).is_err() {
        return;
    }
    if n == 1 || n % 10 == 0 {
        out::stderr_line(NUDGE);
    }
}

/// The markdown hint.
pub const NUDGE: &str = "hint: Thread renders markdown — **bold**, lists, `code`, fences, links.";

/// A word of digits as bash's arithmetic reads it: a leading 0 is octal
/// (`None` for an 8 or 9 there, which bash refuses), 64-bit wrapping.
pub fn bash_arith(digits: &str) -> Option<i64> {
    if digits.len() > 1 && digits.starts_with('0') {
        return digits.bytes().try_fold(0i64, |acc, b| {
            (b < b'8').then(|| acc.wrapping_mul(8).wrapping_add(i64::from(b - b'0')))
        });
    }
    bash_number(digits)
}

fn read(client: &mut Client, closing: &mut Closing, l: &Line) -> Exit {
    let q = query(l);
    let passport = client.passport().is_some();
    let response = match client.send(READ, Call::Request(&[q.as_str()])) {
        Ok(r) => {
            closing.answer(&r, passport);
            held(&r.body)
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Exit::Error;
        }
        Err(e) => return Exit::from_curl(e.curl_code()),
    };
    if l.json && l.one_seq.is_empty() {
        return echo_out(&response);
    }
    shown(&echo(&response), &l.one_seq, l.json, &l.target).print()
}

/// The read's Python on what `echo` handed it, with its three words:
/// `--seq`'s number (or empty), `--json`, the address.
pub fn shown(raw: &[u8], want: &str, as_json: bool, target: &str) -> py::Shown {
    let mut o = py::Shown::default();
    let Some(d) = loads(raw) else {
        o.stdout = raw.to_vec();
        return o;
    };
    let want: i64 = want.parse().unwrap_or(0);
    match shown_with(&d, raw, want, as_json, target, &mut o) {
        Ok(code) => o.exit(code),
        Err(r) => o.raised(&r),
    }
}

fn shown_with(
    d: &Value,
    raw: &[u8],
    want: i64,
    as_json: bool,
    target: &str,
    o: &mut py::Shown,
) -> Result<i32, Raise> {
    if not_ok(d, 10)? {
        if as_json {
            o.line(py::strip(&String::from_utf8_lossy(raw)));
            return Ok(2);
        }
        let (line, _) = py::error_line(d, 14)?;
        o.err(&line);
        return Ok(2);
    }
    let mut list = or(get_at(d, "items", 21)?, Value::Array(Vec::new()));
    if want != 0 {
        let wanted = Value::Int(want.to_string());
        let mut kept = Vec::new();
        for it in super::super::py_iter(&list, 23)? {
            if super::super::msg::py::eq(&get_at(&it, "seq", 23)?, &wanted) {
                kept.push(it);
            }
        }
        if kept.is_empty() {
            o.err(&format!(
                "No message {want} on this Thread ({CLI_NAME} thread {target} lists them as [N]).\n"
            ));
            return Ok(1);
        }
        if as_json {
            let Value::Object(mut pairs) = d.clone() else {
                unreachable!("d.get worked, so d is a dict")
            };
            set(&mut pairs, "items", Value::Array(kept));
            set(&mut pairs, "has_more", Value::Bool(false));
            o.line(&dumps(&Value::Object(pairs)));
            return Ok(0);
        }
        list = Value::Array(kept);
    }
    if !truthy(&list) {
        o.line("(empty)");
        return Ok(0);
    }
    for it in super::super::py_iter(&list, 49)? {
        message(&it, o)?;
    }
    Ok(0)
}

/// `d[key] = v`: in its place when there, else last.
fn set(pairs: &mut Vec<(Text, Value)>, key: &str, v: Value) {
    let k = Text::from(key);
    match pairs.iter_mut().find(|(pk, _)| *pk == k) {
        Some((_, pv)) => *pv = v,
        None => pairs.push((k, v)),
    }
}

/// The break kinds `body_lines` makes new lines of.
const LINE_BREAKS: [&str; 10] = [
    "\r\n", "\r", "\x0b", "\x0c", "\x1c", "\x1d", "\x1e", "\u{85}", "\u{2028}", "\u{2029}",
];

/// `body_lines(s)`, or the `AttributeError` for a body that is no string.
fn body_lines(body: &Value) -> Result<Vec<String>, Raise> {
    let Value::Str(_) = body else {
        return Err(Raise::inside(
            63,
            47,
            "body_lines",
            format!(
                "AttributeError: '{}' object has no attribute 'replace'",
                crate::out::py::type_name(body)
            ),
        ));
    };
    let mut t = str_of(body);
    for b in LINE_BREAKS {
        t = t.replace(b, "\n");
    }
    Ok(t.split('\n').map(str::to_string).collect())
}

/// One message's lines.
fn message(it: &Value, o: &mut py::Shown) -> Result<(), Raise> {
    let doc = or(get_at(it, "doc", 50)?, Value::Object(Vec::new()));
    let mut body = or(get_at(&doc, "body", 51)?, s(""));
    if matches!(get_at(&doc, "deleted", 52)?, Value::Bool(true)) {
        body = s("(message deleted)");
    }
    let mut frm = or(get_at(&doc, "from", 56)?, s(""));
    let author = match get_at(&doc, "author", 57)? {
        a @ Value::Object(_) => a,
        _ => Value::Object(Vec::new()),
    };
    if super::super::msg::py::eq(&get_at(&author, "kind", 58)?, &s("guest")) {
        let name = or(
            or(
                or(get_at(&author, "name", 61)?, get_at(&author, "login", 61)?),
                frm.clone(),
            ),
            s("?"),
        );
        frm = s(&format!("guest {}", str_of(&name)));
    }
    let seq = get_at(it, "seq", 62)?;
    let lines = body_lines(&body)?;
    let frm = str_of(&frm);
    o.line(&format!("[{}] from {frm}: {}", str_of(&seq), lines[0]));
    for l in &lines[1..] {
        o.line(&format!("  | {l}"));
    }
    if let ctx @ Value::Object(_) = get_at(&doc, "context", 67)? {
        if let text @ Value::Str(_) = get_at(&ctx, "text", 68)? {
            let place = get_or(&ctx, "placement", s(""), 70)?;
            o.line(&format!(
                "  [context from Garden widget {}, not typed by {frm}] {}",
                str_of(&place),
                dumps(&text)
            ));
        }
    }
    for a in items(get_at(&doc, "attachments", 71)?, 71)? {
        let st = or(get_at(&a, "state", 72)?, s("ok"));
        if super::super::msg::py::eq(&st, &s("hidden")) {
            o.line("  [file] (not shared with you)");
            continue;
        }
        let line = line_spec(&a, 77)?;
        let tag = if super::super::msg::py::eq(&st, &s("ok")) {
            String::new()
        } else {
            format!(" ({})", str_of(&st))
        };
        let path = get_or(&a, "path", s(""), 80)?;
        let sz = size(&get_at(&a, "size", 80)?, 80)?;
        o.line(&format!("  [file] {}{line} ({sz}){tag}", str_of(&path)));
    }
    Ok(())
}
