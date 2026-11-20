//! `k5 organization <verb> ...`: the Org Chart's one tool (orgs, heads,
//! lines and the daily bulletin), as the shell CLI's `cmd_organization`
//! does it (`cli/src/permission.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `organization` is an `id` tool: in a session (`HOOK_SOCK` set) a
//!    passport the daemon no longer knows with its socket gone, and no
//!    passport at all, are refused with exit 3
//!    ([`crate::prelude::id_tool`]). The shared Codex server check that
//!    comes first is the shell's (a hand-back).
//! 3. The words after the verb, as the shell's loop reads them ([`flags`]):
//!    `--json`, `--peek`, `--file`, `--why`, `--purpose`, `--member` and
//!    `--org` (each also as `--flag=value`), `--since` (no `=` form),
//!    `--` (every word after it is a word), any other `--…` is
//!    `unknown flag`; every other word is a word. A stop is
//!    `Error: <why>` and `See: k2 organization --help`, exit 2.
//! 4. The verb ([`plan`]): its checks (each a stop as above), then one
//!    `cli_send_json` ([`crate::transport::Client::send_json`]): a GET
//!    with an escaped query, or a POST with a JSON body built as the
//!    shell's `_org_json` builds it ([`org_json`]). An unknown verb is
//!    `Unknown organization command: <verb>` and the help on stderr,
//!    exit 2.
//! 5. `_org_call`: no answer (the shell's status `000`) is one line and
//!    exit 1; else the shell's Python reads the answer ([`render`]): an
//!    answer that is no JSON object is one line (exit by the status), a
//!    refusal or an error is the daemon's error on stderr (3 for 401 and
//!    403, 2 for another 4xx, else 1), `--json` (and `chart`) prints the
//!    object, else the verb's text. `reach` held for a person (a card, a
//!    ticket or an open one) is exit 2.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]): the refusal's
//!    closing line when the command failed, the bulletin line when it
//!    worked.
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`), so
//! there is one copy of each: `organization` alone, `organization help`
//! and any form with `-h` or `--help` (the help, which the front sends to
//! a page or the shell, [`crate::front`]); a word that is not UTF-8; a
//! port that is set but is no port curl takes; a `python3` the binary
//! can't tell ([`crate::families`], Without Python); the shared Codex
//! server check; and `bulletin set --file <path>` when the file can't be
//! read, or holds a NUL or bytes that are not UTF-8 (bash's own `cat`
//! line or warning, and Python's escapes).
//!
//! Where this differs from the shell: an answer that is not UTF-8 is read
//! with U+FFFD for each bad byte, and a string with half of a surrogate
//! pair is printed as `\udXXX`, as [`super::inbox`] says. The daemon
//! sends neither. Where the shell's Python raises, the traceback is
//! Python 3.12's, without source lines.

use super::{curl_port, get, hook_sock, py_iter, py_len, raised, without_nuls, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::remind::shared::{
    at_line as at_module, dget, join, join_items, or, plus_str, sub,
};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::Error;
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["organization"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's tool policy.
pub const TOOL: &str = "organization";

/// The routes' head.
pub const ROUTE: &str = "/cli/organization";

/// `_org_call`'s line when the daemon gave no answer (status `000`).
pub const NO_ANSWER: &str = concat!(
    "Error: no answer from the ",
    k2_client::product_name!(),
    " daemon (is ",
    k2_client::product_name!(),
    concat!(" running? ", cli_name!(), " daemon status)")
);

/// `_org_usage_err`'s second line.
pub const SEE_HELP: &str = concat!("See: ", cli_name!(), " organization --help");

/// The flags and words after the verb.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Flags {
    pub json: bool,
    pub peek: bool,
    pub file: String,
    pub why: String,
    pub purpose: String,
    pub member: String,
    pub org: String,
    pub since: String,
    pub args: Vec<String>,
}

/// The words after the verb, read as the shell's loop reads them; a stop
/// is `_org_usage_err`'s text.
pub fn flags(words: &[&str]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut i = 0;
    // `--flag value` (the next word, whatever it is) or `--flag=value`.
    let value = |i: usize, why: &str| -> Result<String, String> {
        words
            .get(i + 1)
            .map(|w| (*w).to_string())
            .ok_or_else(|| why.to_string())
    };
    while i < words.len() {
        let a = words[i];
        let mut step = 1;
        match a {
            "--json" => f.json = true,
            "--peek" => f.peek = true,
            "--file" => {
                f.file = value(i, "--file needs a path")?;
                step = 2;
            }
            "--why" => {
                f.why = value(i, "--why needs one line")?;
                step = 2;
            }
            "--purpose" => {
                f.purpose = value(i, "--purpose needs text")?;
                step = 2;
            }
            "--member" => {
                f.member = value(i, "--member needs an agent")?;
                step = 2;
            }
            "--org" => {
                f.org = value(i, "--org needs an org")?;
                step = 2;
            }
            "--since" => {
                f.since = value(i, "--since needs a unix time")?;
                step = 2;
            }
            "--" => {
                f.args
                    .extend(words[i + 1..].iter().map(|w| (*w).to_string()));
                break;
            }
            _ => {
                if let Some(v) = a.strip_prefix("--file=") {
                    f.file = v.to_string();
                } else if let Some(v) = a.strip_prefix("--why=") {
                    f.why = v.to_string();
                } else if let Some(v) = a.strip_prefix("--purpose=") {
                    f.purpose = v.to_string();
                } else if let Some(v) = a.strip_prefix("--member=") {
                    f.member = v.to_string();
                } else if let Some(v) = a.strip_prefix("--org=") {
                    f.org = v.to_string();
                } else if a.starts_with("--") {
                    return Err(format!("unknown flag '{a}'"));
                } else {
                    f.args.push(a.to_string());
                }
            }
        }
        i += step;
    }
    Ok(f)
}

/// What the verb sends, and how its answer is printed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The printer's verb (`K2_OV`).
    pub verb: &'static str,
    pub post: bool,
    pub path: String,
    /// A GET's escaped query, or a POST's JSON body.
    pub payload: String,
    /// `--json`, or `chart`.
    pub json: bool,
}

/// Why a verb sends nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// `_org_usage_err <why>`: two lines, exit 2.
    Usage(String),
    /// `Unknown organization command: <verb>` and the help, exit 2.
    Unknown(String),
}

impl Stop {
    /// Prints the lines on stderr: the exit.
    pub fn print(&self) -> Exit {
        match self {
            Stop::Usage(why) => {
                out::stderr_line(&format!("Error: {why}"));
                out::stderr_line(SEE_HELP);
            }
            Stop::Unknown(verb) => {
                out::stderr_line(&format!("Unknown organization command: {verb}"));
                let help = crate::text::help("organization").unwrap_or_default();
                let _ = out::write_flushed(&mut std::io::stderr().lock(), help.as_bytes());
            }
        }
        Exit::Usage
    }
}

fn usage<T>(why: impl Into<String>) -> Result<T, Stop> {
    Err(Stop::Usage(why.into()))
}

/// `_org_json k=v ...`: a JSON object of the pairs, each value a string,
/// a key ending in `[]` a list of its values; as Python's `json.dumps`
/// writes it.
pub fn org_json(kv: &[(&str, &str)]) -> String {
    let mut pairs: Vec<(Text, Value)> = Vec::new();
    for (k, v) in kv {
        let value = super::s(v);
        if let Some(list) = k.strip_suffix("[]") {
            let key = Text::from(list);
            match pairs.iter_mut().find(|(have, _)| *have == key) {
                Some((_, Value::Array(items))) => items.push(value),
                Some((_, other)) => *other = Value::Array(vec![value]),
                None => pairs.push((key, Value::Array(vec![value]))),
            }
        } else {
            let key = Text::from(*k);
            match pairs.iter_mut().find(|(have, _)| *have == key) {
                Some((_, other)) => *other = value,
                None => pairs.push((key, value)),
            }
        }
    }
    json::to_string(&Value::Object(pairs), &Options::default()).unwrap_or_default()
}

fn get_plan(verb: &'static str, path: &str, query: String, json: bool) -> Plan {
    Plan {
        verb,
        post: false,
        path: format!("{ROUTE}{path}"),
        payload: query,
        json,
    }
}

fn post_plan(verb: &'static str, path: &str, kv: &[(&str, &str)], json: bool) -> Plan {
    Plan {
        verb,
        post: true,
        path: format!("{ROUTE}{path}"),
        payload: org_json(kv),
        json,
    }
}

/// `bulletin set`'s file, when it names one: `None` for no `--file`,
/// else whether `[ -f ]` holds and the text `$(cat …)` gives (its trailing
/// newlines gone).
pub type FileText = Option<Option<String>>;

/// The verb's checks and its request. `file`: `bulletin set`'s file
/// (read before anything runs, [`file_text`]).
pub fn plan(verb: &str, f: &Flags, file: &FileText) -> Result<Plan, Stop> {
    let a = &f.args;
    let n = a.len();
    let arg = |i: usize| a.get(i).map(String::as_str).unwrap_or("");
    let joined = |from: usize| a.get(from..).map(|r| r.join(" ")).unwrap_or_default();
    let j = f.json;
    Ok(match verb {
        "bulletin" => match arg(0) {
            "" if f.peek => get_plan("bulletin", "/bulletin", "peek=1".into(), j),
            "" => get_plan("bulletin", "/bulletin", String::new(), j),
            "read" => post_plan("bulletin-read", "/bulletin/read", &[], j),
            "show" => {
                let o = if arg(1).is_empty() { f.org.as_str() } else { arg(1) };
                if o.is_empty() {
                    return usage(concat!("name the org: ", cli_name!(), " organization bulletin show <org>"));
                }
                get_plan("show", "/show", format!("org={}", quote(o)), j)
            }
            "set" => {
                let mut lines: Vec<String> = Vec::new();
                if !f.file.is_empty() {
                    match file {
                        Some(Some(text)) => lines.push(text.clone()),
                        _ => return usage(format!("no file '{}'", f.file)),
                    }
                }
                lines.extend(a.iter().skip(1).cloned());
                if lines.is_empty() {
                    return usage(
                        concat!("give the lines: ", cli_name!(), " organization bulletin set \"<line 1>\" … (or --file <path>)"),
                    );
                }
                let mut kv: Vec<(&str, &str)> =
                    lines.iter().map(|l| ("lines[]", l.as_str())).collect();
                if !f.org.is_empty() {
                    kv.push(("org", f.org.as_str()));
                }
                post_plan("bulletin-set", "/bulletin/set", &kv, j)
            }
            _ => return usage("bulletin takes no argument, or read, set or show"),
        },
        "status" => get_plan("status", "/bulletin/status", String::new(), j),
        "list" => get_plan("list", "/list", String::new(), j),
        "show" => {
            if n < 1 {
                return usage(concat!("name the org: ", cli_name!(), " organization show <org>"));
            }
            get_plan("show", "/show", format!("org={}", quote(arg(0))), j)
        }
        "mine" => get_plan("mine", "/mine", String::new(), j),
        "night" => get_plan("night", "/night", String::new(), j),
        "chart" => get_plan("chart", "/chart", String::new(), true),
        "reach" => {
            if n < 1 {
                return usage(
                    concat!("name the org (or its head): ", cli_name!(), " organization reach <org> --why \"<one line>\""),
                );
            }
            if f.why.is_empty() {
                return usage(format!(
                    concat!("say why in one line: ", cli_name!(), " organization reach {} --why \"<one line>\""),
                    arg(0)
                ));
            }
            post_plan("reach", "/reach", &[("org", arg(0)), ("why", f.why.as_str())], j)
        }
        "updates" => {
            let mut q = String::new();
            if !f.org.is_empty() {
                q = format!("org={}", quote(&f.org));
            }
            if !f.since.is_empty() {
                if !q.is_empty() {
                    q.push('&');
                }
                q.push_str(&format!("since={}", quote(&f.since)));
            }
            get_plan("updates", "/updates", q, j)
        }
        "ask" => {
            if n < 1 {
                return usage(
                    concat!("give the question: ", cli_name!(), " organization ask \"<question>\" [--member <agent>]"),
                );
            }
            let q = joined(0);
            let mut kv = vec![("question", q.as_str())];
            if !f.member.is_empty() {
                kv.push(("member", f.member.as_str()));
            }
            post_plan("ask", "/ask", &kv, j)
        }
        "update" => {
            if n < 1 {
                return usage(concat!("give the news: ", cli_name!(), " organization update \"<text>\""));
            }
            let t = joined(0);
            let mut kv = vec![("text", t.as_str())];
            if !f.org.is_empty() {
                kv.push(("org", f.org.as_str()));
            }
            post_plan("update", "/update", &kv, j)
        }
        "create" => {
            if n < 1 {
                return usage(concat!("name the org: ", cli_name!(), " organization create <name> [--purpose \"…\"]"));
            }
            let name = joined(0);
            post_plan(
                "create",
                "/create",
                &[("name", name.as_str()), ("purpose", f.purpose.as_str())],
                j,
            )
        }
        "rename" => {
            if n < 2 {
                return usage(concat!(cli_name!(), " organization rename <org> <new name>"));
            }
            let name = joined(1);
            post_plan("rename", "/rename", &[("org", arg(0)), ("name", name.as_str())], j)
        }
        "purpose" => {
            if n < 1 {
                return usage(concat!(cli_name!(), " organization purpose <org> \"<short paragraph>\""));
            }
            let mut p = joined(1);
            if !f.purpose.is_empty() {
                p = f.purpose.clone();
            }
            post_plan("purpose", "/purpose", &[("org", arg(0)), ("purpose", p.as_str())], j)
        }
        "delete" => {
            if n != 1 {
                return usage(concat!(cli_name!(), " organization delete <org>"));
            }
            post_plan("delete", "/delete", &[("org", arg(0))], j)
        }
        "add" => {
            if n != 2 {
                return usage(concat!(cli_name!(), " organization add <org> <agent>"));
            }
            post_plan("add", "/member/add", &[("org", arg(0)), ("agent", arg(1))], j)
        }
        "move" => {
            if n != 2 {
                return usage(concat!(cli_name!(), " organization move <agent> <org>"));
            }
            post_plan("move", "/member/move", &[("agent", arg(0)), ("org", arg(1))], j)
        }
        "remove" => {
            if n != 2 {
                return usage(concat!(cli_name!(), " organization remove <org> <agent>"));
            }
            post_plan(
                "remove",
                "/member/remove",
                &[("org", arg(0)), ("agent", arg(1))],
                j,
            )
        }
        "head" => {
            if n != 2 {
                return usage(concat!(cli_name!(), " organization head <org> <agent|none>"));
            }
            post_plan("head", "/head", &[("org", arg(0)), ("agent", arg(1))], j)
        }
        "link" => {
            if n != 3 {
                return usage(concat!(cli_name!(), " organization link <a> above|below|beside <b>"));
            }
            // The route's kinds: below (a over b), under (a under b), beside.
            let kind = match arg(1) {
                "above" => "below",
                "below" => "under",
                "beside" => "beside",
                _ => {
                    return usage("say how they sit: above (a over b), below (a under b) or beside. Never top to top.")
                }
            };
            post_plan(
                "link",
                "/line/add",
                &[("a", arg(0)), ("kind", kind), ("b", arg(2))],
                j,
            )
        }
        "unlink" => {
            if n != 2 {
                return usage(concat!(cli_name!(), " organization unlink <a> <b>"));
            }
            post_plan("unlink", "/line/remove", &[("a", arg(0)), ("b", arg(1))], j)
        }
        "position" | "place" | "layout" | "arrange" => {
            return usage("there is no position verb: the chart lays itself out (a person drags cards on the Org Chart tab)")
        }
        other => return Err(Stop::Unknown(other.to_string())),
    })
}

/// `bulletin set --file <path>`'s file, read before anything is printed
/// or sent: `Some(None)` when `[ -f ]` fails, `Some(Some(text))` with its
/// trailing newlines gone. `None` (in the outer `Option` of the result):
/// the shell answers it (a file `cat` can't read, or one with a NUL or
/// bytes that are not UTF-8).
pub fn file_text(verb: &str, f: &Flags) -> Option<FileText> {
    if verb != "bulletin" || f.args.first().map(String::as_str) != Some("set") || f.file.is_empty()
    {
        return Some(None);
    }
    let path = Path::new(&f.file);
    if !path.is_file() {
        return Some(Some(None));
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let mut text = String::from_utf8(bytes).ok()?;
    while text.ends_with('\n') {
        text.pop();
    }
    Some(Some(Some(text)))
}

/// `k5 organization <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let (verb, rest) = words.split_first()?;
    // The help, alone or anywhere: the front's page or the shell's.
    if matches!(*verb, "" | "help" | "-h" | "--help")
        || rest.iter().any(|w| matches!(*w, "-h" | "--help"))
    {
        return None;
    }
    let read = flags(rest);
    let file = match &read {
        Ok(f) => file_text(verb, f)?,
        Err(_) => None,
    };
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
    Some(run_with(
        args,
        verb,
        read,
        &file,
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    args: &[OsString],
    verb: &str,
    read: Result<Flags, String>,
    file: &FileText,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let plan = match read.map_err(Stop::Usage).and_then(|f| plan(verb, &f, file)) {
        Ok(p) => p,
        Err(stop) => return stop.print(),
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
    let passport = client.passport().is_some();
    let mut closing = Closing::default();
    let exit = match client.send_json(plan.post, &plan.path, &plan.payload) {
        Ok(r) => {
            closing.answer(&r, passport);
            render(plan.verb, r.status, &without_nuls(&r.body), plan.json).print()
        }
        Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(_) => {
            out::stderr_line(NO_ANSWER);
            Exit::Error
        }
    };
    closing.flush(&mut client, exit);
    exit
}

/// The lines of the shell's Python (`_ORG_PY`) a traceback names.
pub mod line {
    pub const WRITE_TEXT: u32 = 59;
    pub const LIST_ORGS: u32 = 68;
    pub const LIST_LINES: u32 = 69;
    pub const SHOW_LINES: u32 = 72;
    pub const SHOW_FOR: u32 = 73;
    pub const SHOW_KIND: u32 = 74;
    pub const SHOW_LINE: u32 = 75;
    pub const MINE_NEXT: u32 = 79;
    pub const MINE_HEAD: u32 = 83;
    pub const MINE_NO_HEAD: u32 = 85;
    pub const MINE_CONNECTED: u32 = 87;
    pub const MINE_NOT_CONNECTED: u32 = 89;
    pub const MINE_LINES: u32 = 90;
    pub const REACH: u32 = 92;
    pub const NIGHT_MACHINE: u32 = 95;
    pub const NIGHT_FREE: u32 = 100;
    pub const NIGHT_FREE_GIB: u32 = 104;
    pub const NIGHT_LOAD: u32 = 105;
    pub const NIGHT_COUNTS: u32 = 109;
    pub const NIGHT_ORGS: u32 = 111;
    pub const NIGHT_STATE: u32 = 112;
    pub const NIGHT_ROW: u32 = 117;
    pub const UPDATES_NONE: u32 = 121;
    pub const UPDATES_FOR: u32 = 122;
    pub const UPDATES_ROW: u32 = 123;
    pub const ASK_ASKED: u32 = 126;
    pub const ASK_QUEUED: u32 = 128;
    pub const ASK_FOR: u32 = 130;
    pub const ASK_ROW: u32 = 131;
    pub const UPDATE_HEAD: u32 = 133;
    pub const SET_LEN: u32 = 139;
    pub const SET_ORG: u32 = 140;
    pub const LINK_KIND: u32 = 143;
    pub const LINK_ROW: u32 = 144;
    pub const DELETE: u32 = 148;
    pub const CHANGED_LINES: u32 = 151;
    pub const ORG_LINES: u32 = 153;
    /// Inside `org_lines`.
    pub const OL_HEAD: u32 = 39;
    pub const OL_PURPOSE: u32 = 41;
    pub const OL_PURPOSE_ROW: u32 = 42;
    pub const OL_MEMBERS: u32 = 43;
    pub const OL_JOIN: u32 = 45;
    pub const OL_FOR_ROLE: u32 = 46;
    pub const OL_ROLE: u32 = 47;
    pub const OL_ROLE_ROW: u32 = 48;
    pub const OL_BULLETIN: u32 = 51;
    pub const OL_LINES: u32 = 52;
    pub const OL_FOR_LINES: u32 = 54;
}

/// `code_for(st)`.
pub fn code_for(status: u16) -> Exit {
    match status {
        200..=299 => Exit::Ok,
        401 | 403 => Exit::Refused,
        400..=499 => Exit::Usage,
        _ => Exit::Error,
    }
}

/// `raw[:300]`.
fn head300(raw: &str) -> String {
    raw.chars().take(300).collect()
}

/// What the printer writes so far and where it raises.
struct Py {
    out: String,
}

type R<T> = Result<T, Raise>;

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

fn none() -> Value {
    Value::Null
}

fn str_or_none(v: Option<&Value>) -> String {
    py::str_of(v.unwrap_or(&Value::Null))
}

/// `'%.1f' % x` for a number `isinstance(x, (int, float))` takes, or the
/// `OverflowError` of an int too large for a float.
fn tenths(v: &Value, divide: bool) -> Result<String, String> {
    let x = match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Int(d) => {
            let f: f64 = d.parse().unwrap_or(f64::INFINITY);
            if f.is_infinite() {
                return Err("OverflowError: int too large to convert to float".into());
            }
            f
        }
        Value::Float(f) => *f,
        _ => return Ok(String::new()),
    };
    let x = if divide { x / 1_073_741_824.0 } else { x };
    Ok(if x.is_nan() {
        "nan".to_string()
    } else {
        // Rust rounds the exact value half to even, as Python does; an
        // infinity is `inf` in both.
        format!("{x:.1}")
    })
}

fn number(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| matches!(x, Value::Bool(_) | Value::Int(_) | Value::Float(_)))
}

impl Py {
    fn w(&mut self, s: &str) {
        self.out.push_str(s);
    }

    /// `w(v)`: `sys.stdout.write` takes only a string.
    fn write_value(&mut self, v: &Value, line: u32) -> R<()> {
        match v {
            Value::Str(_) => {
                self.w(&py::str_of(v));
                Ok(())
            }
            other => Err(Raise::at(
                line,
                format!(
                    "TypeError: write() argument must be str, not {}",
                    py::type_name(other)
                ),
            )),
        }
    }

    /// `org_lines(o)`, called from line `from`.
    fn org_lines(&mut self, o: &Value, from: u32) -> R<()> {
        let at = |line: u32| move |e: String| Raise::inside(from, line, "org_lines", e);
        let head = dget(o, "head", &at(line::OL_HEAD))?;
        let shown = match head {
            Some(h) if py::truthy(h) => match h {
                Value::Object(p) => str_or_none(get(p, "handle")),
                _ => "none yet".to_string(),
            },
            _ => "none yet".to_string(),
        };
        self.w(&format!(
            "{} · head: {}\n",
            str_or_none(get_obj(o, "name")),
            shown
        ));
        if get_obj(o, "purpose").is_some_and(py::truthy) {
            let p = get_obj(o, "purpose").unwrap_or(&Value::Null);
            self.w(&format!("  Purpose: {}\n", py::str_of(p)));
        }
        let ms = or(get_obj(o, "members"), Value::Array(Vec::new()));
        if py::truthy(&ms) {
            let join_at = at(line::OL_JOIN);
            let gen_at = |e: String| Raise {
                frames: vec![
                    (from, "<module>"),
                    (line::OL_JOIN, "org_lines"),
                    (line::OL_JOIN, "<genexpr>"),
                ],
                exception: e,
            };
            let members = py_iter(&ms, 0).map_err(|r| join_at(r.exception))?;
            let mut parts = Vec::new();
            for m in &members {
                let is_head = dget(m, "isHead", &gen_at)?.is_some_and(py::truthy);
                let handle = sub(m, "handle", &gen_at)?;
                parts.push(if is_head {
                    super::s(&format!("{} (head)", py::str_of(handle)))
                } else {
                    handle.clone()
                });
            }
            let joined = join_items(&parts, &join_at)?;
            self.w(&format!("  Members: {joined}\n"));
            for m in &members {
                let role_at = at(line::OL_ROLE);
                if dget(m, "role", &role_at)?.is_some_and(py::truthy) {
                    let row_at = at(line::OL_ROLE_ROW);
                    let h = sub(m, "handle", &row_at)?;
                    let r = sub(m, "role", &row_at)?;
                    self.w(&format!("    {}: {}\n", py::str_of(h), py::str_of(r)));
                }
            }
        } else {
            self.w("  Members: none yet\n");
        }
        let bl = get_obj(o, "bulletin").cloned().unwrap_or(Value::Null);
        let has_lines =
            py::truthy(&bl) && dget(&bl, "lines", &at(line::OL_LINES))?.is_some_and(py::truthy);
        if has_lines {
            self.w("  Bulletin:\n");
            let lines = sub(&bl, "lines", &at(line::OL_FOR_LINES))?;
            let items = py_iter(lines, 0).map_err(|r| at(line::OL_FOR_LINES)(r.exception))?;
            for l in &items {
                self.w(&format!("    {}\n", py::str_of(l)));
            }
        } else {
            self.w("  Bulletin: none yet\n");
        }
        Ok(())
    }
}

/// `o.get(key)` where `o` is a dict for sure (an earlier `get` held).
fn get_obj<'a>(o: &'a Value, key: &str) -> Option<&'a Value> {
    match o {
        Value::Object(p) => get(p, key),
        _ => None,
    }
}

/// The shell's Python (`_ORG_PY`) on an answer: `body` is what `$(…)`
/// held of it (NULs gone), `status` its HTTP status.
pub fn render(verb: &str, status: u16, body: &[u8], as_json: bool) -> Shown {
    let raw = String::from_utf8_lossy(body).into_owned();
    let rc = code_for(status);
    let v = json::parse(&raw).ok();
    let Some(Value::Object(mut pairs)) = v else {
        return Shown::err(
            format!(
                concat!(
                    "Error: unexpected answer from the ",
                    k2_client::product_name!(),
                    " daemon (HTTP {}): {}\n"
                ),
                status,
                head300(&raw)
            ),
            if rc == Exit::Ok { Exit::Error } else { rc },
        );
    };
    if let Some(Value::Object(b)) = get(&pairs, "bulletin") {
        if get(b, "unread").is_some() {
            pairs.retain(|(k, _)| *k != Text::from("bulletin"));
        }
    }
    let v = Value::Object(pairs);
    if rc != Exit::Ok {
        let e = get_obj(&v, "error");
        let line = match e {
            Some(Value::Object(ep)) => {
                let code = get(ep, "code")
                    .cloned()
                    .unwrap_or_else(|| super::s("error"));
                let shown = [get(ep, "hint"), get(ep, "message")]
                    .into_iter()
                    .flatten()
                    .find(|x| py::truthy(x))
                    .cloned()
                    .unwrap_or_else(|| super::s(""));
                format!("Error [{}]: {}\n", py::str_of(&code), py::str_of(&shown))
            }
            _ => {
                let shown = [get_obj(&v, "message"), e]
                    .into_iter()
                    .flatten()
                    .find(|x| py::truthy(x))
                    .cloned()
                    .unwrap_or_else(|| super::s(&head300(&raw)));
                format!("Error: {}\n", py::str_of(&shown))
            }
        };
        return Shown::err(line, rc);
    }
    let held = verb == "reach"
        && get_obj(&v, "status")
            .is_some_and(|s| ["card", "ticket", "open"].iter().any(|w| *s == super::s(w)));
    let done = if held { Exit::Usage } else { Exit::Ok };
    if as_json {
        let text = json::to_string(&v, &Options::default()).unwrap_or_default();
        return Shown {
            stdout: text + "\n",
            stderr: String::new(),
            exit: done,
        };
    }
    let mut p = Py { out: String::new() };
    match text(&mut p, verb, &v) {
        Ok(()) => Shown {
            stdout: p.out,
            stderr: String::new(),
            exit: done,
        },
        Err(r) => raised(p.out, String::new(), &r),
    }
}

/// The verbs' text, from `w = sys.stdout.write` on.
fn text(p: &mut Py, verb: &str, v: &Value) -> R<()> {
    let g = |key: &str| get_obj(v, key);
    match verb {
        "bulletin" => {
            let t = g("text").cloned().unwrap_or_else(|| super::s(""));
            p.write_value(&t, line::WRITE_TEXT)?;
        }
        "bulletin-read" => {}
        "status" => {
            let word = if g("unread").is_some_and(py::truthy) {
                concat!("unread (read it: ", cli_name!(), " organization bulletin)")
            } else {
                "read"
            };
            p.w(&format!("{}: {word}\n", str_or_none(g("day"))));
        }
        "list" => {
            let orgs = or(g("orgs"), Value::Array(Vec::new()));
            if !py::truthy(&orgs) {
                p.w(concat!(
                    "No orgs yet. An Owner or Admin makes one: ",
                    cli_name!(),
                    " organization create <name>\n"
                ));
            }
            for o in py_iter(&orgs, line::LIST_ORGS)? {
                p.org_lines(&o, line::LIST_LINES)?;
            }
        }
        "show" => {
            let o = or(g("org"), empty_dict());
            p.org_lines(&o, line::SHOW_LINES)?;
            let lines = or(
                dget(&o, "lines", &at_module(line::SHOW_FOR))?,
                Value::Array(Vec::new()),
            );
            for l in py_iter(&lines, line::SHOW_FOR)? {
                let kind = dget(&l, "kind", &at_module(line::SHOW_KIND))?;
                let word = if kind.is_some_and(|k| *k == super::s("below")) {
                    "above"
                } else {
                    "beside"
                };
                let at = at_module(line::SHOW_LINE);
                let a = sub(sub(&l, "a", &at)?, "name", &at)?;
                let b = sub(sub(&l, "b", &at)?, "name", &at)?;
                p.w(&format!(
                    "  Line: {} {word} {}\n",
                    py::str_of(a),
                    py::str_of(b)
                ));
            }
        }
        "mine" => {
            let o = g("org").cloned().unwrap_or(Value::Null);
            if !py::truthy(&o) {
                let next = g("next")
                    .cloned()
                    .unwrap_or_else(|| super::s("You are in no org."));
                let line = plus_str(&next, "\n", &at_module(line::MINE_NEXT))?;
                p.w(&line);
            } else {
                let head = g("head").cloned().unwrap_or(Value::Null);
                if g("isHead").is_some_and(py::truthy) {
                    let at = at_module(line::MINE_HEAD);
                    let name = plus_str(sub(&o, "name", &at)?, "'s", &at)?;
                    p.w(&format!("You are {name} head.\n"));
                } else if !py::truthy(&head) {
                    let name = sub(&o, "name", &at_module(line::MINE_NO_HEAD))?;
                    p.w(&format!(
                        "You are in {}. It has no head yet.\n",
                        py::str_of(name)
                    ));
                } else if g("connectedToHead").is_some_and(py::truthy) {
                    let at = at_module(line::MINE_CONNECTED);
                    let name = sub(&o, "name", &at)?;
                    let h = sub(&head, "handle", &at)?;
                    p.w(&format!(
                        concat!(
                            "You are in {}. Head: {} (connected: ",
                            cli_name!(),
                            " msg {} \"...\").\n"
                        ),
                        py::str_of(name),
                        py::str_of(h),
                        py::str_of(h)
                    ));
                } else {
                    let at = at_module(line::MINE_NOT_CONNECTED);
                    let name = sub(&o, "name", &at)?;
                    let h = sub(&head, "handle", &at)?;
                    p.w(&format!(
                        concat!(
                            "You are in {}. Head: {} (not connected: ",
                            cli_name!(),
                            " organization reach {} --why \"...\").\n"
                        ),
                        py::str_of(name),
                        py::str_of(h),
                        py::str_of(name)
                    ));
                }
                p.org_lines(&o, line::MINE_LINES)?;
            }
        }
        "reach" => {
            let t = or(g("text"), super::s(""));
            let line = plus_str(&t, "\n", &at_module(line::REACH))?;
            p.w(&line);
        }
        "night" => night(p, v)?,
        "updates" => {
            let us = or(g("updates"), Value::Array(Vec::new()));
            if !py::truthy(&us) {
                let org = or(g("org"), empty_dict());
                let name = dget(&org, "name", &at_module(line::UPDATES_NONE))?
                    .cloned()
                    .unwrap_or_else(|| super::s("your org"));
                p.w(&format!(
                    "No updates for {} since last night.\n",
                    py::str_of(&name)
                ));
            }
            for u in py_iter(&us, line::UPDATES_FOR)? {
                let at = at_module(line::UPDATES_ROW);
                let from = sub(sub(&u, "from", &at)?, "handle", &at)?;
                let body = sub(&u, "body", &at)?;
                p.w(&format!("{}: {}\n", py::str_of(from), py::str_of(body)));
            }
        }
        "ask" => {
            let q = or(g("queued"), Value::Array(Vec::new()));
            let asked = join(
                &or(g("asked"), Value::Array(Vec::new())),
                &at_module(line::ASK_ASKED),
            )?;
            let asked = if asked.is_empty() {
                "nobody".to_string()
            } else {
                asked
            };
            p.w(&format!("Asked {asked} now.\n"));
            if py::truthy(&q) {
                let queued = join(&q, &at_module(line::ASK_QUEUED))?;
                let spacing = or(g("spacing"), super::s("one at a time"));
                p.w(&format!(
                    concat!("Queued: {} (", k2_client::product_name!(), " asks {}).\n"),
                    queued,
                    py::str_of(&spacing)
                ));
            }
            p.w(concat!(
                "Their answers: ",
                cli_name!(),
                " organization updates\n"
            ));
            for n in py_iter(
                &or(g("notDelivered"), Value::Array(Vec::new())),
                line::ASK_FOR,
            )? {
                let at = at_module(line::ASK_ROW);
                let member = dget(&n, "member", &at)?;
                let why = dget(&n, "why", &at)?;
                p.w(&format!(
                    "Not delivered to {} ({}); the ask is stored.\n",
                    str_or_none(member),
                    str_or_none(why)
                ));
            }
        }
        "update" => {
            let head = or(g("head"), empty_dict());
            let h = dget(&head, "handle", &at_module(line::UPDATE_HEAD))?
                .cloned()
                .unwrap_or_else(|| super::s("the head"));
            if g("deliveredNow").is_some_and(py::truthy) {
                p.w(&format!("Sent to {}.\n", py::str_of(&h)));
            } else {
                p.w(&format!(
                    concat!(
                        "Stored for {} (heads read updates at night: ",
                        cli_name!(),
                        " organization updates).\n"
                    ),
                    py::str_of(&h)
                ));
            }
        }
        "bulletin-set" => {
            let n = py_len(&or(g("lines"), Value::Array(Vec::new())), line::SET_LEN)?;
            let at = at_module(line::SET_ORG);
            let name = plus_str(sub(sub(v, "org", &at)?, "name", &at)?, "'s", &at)?;
            p.w(&format!(
                "{name} bulletin now has {n} line{}; every agent sees it at once.\n",
                if n == 1 { "" } else { "s" }
            ));
        }
        "link" => {
            let l = or(g("line"), empty_dict());
            let kind = dget(&l, "kind", &at_module(line::LINK_KIND))?;
            let word = if kind.is_some_and(|k| *k == super::s("below")) {
                "above"
            } else {
                "beside"
            };
            let at = at_module(line::LINK_ROW);
            let a = dget(&l, "a", &at)?.cloned().unwrap_or_else(empty_dict);
            let a = dget(&a, "name", &at)?;
            let b = dget(&l, "b", &at)?.cloned().unwrap_or_else(empty_dict);
            let b = dget(&b, "name", &at)?;
            p.w(&format!(
                "Linked: {} {word} {}.\n",
                str_or_none(a),
                str_or_none(b)
            ));
        }
        "unlink" => p.w("Unlinked.\n"),
        "delete" => {
            let at = at_module(line::DELETE);
            let name = sub(sub(v, "deleted", &at)?, "name", &at)?;
            p.w(&format!(
                "Deleted {}; its members left (the agents stay).\n",
                py::str_of(name)
            ));
        }
        "add" | "remove" if g("changed") == Some(&Value::Bool(false)) => {
            p.w("No change.\n");
            p.org_lines(&or(g("org"), empty_dict()), line::CHANGED_LINES)?;
        }
        "create" | "rename" | "purpose" | "add" | "move" | "remove" | "head" => {
            p.org_lines(&or(g("org"), empty_dict()), line::ORG_LINES)?;
        }
        _ => {
            let text = json::to_string(v, &Options::default()).unwrap_or_default();
            p.w(&(text + "\n"));
        }
    }
    Ok(())
}

/// `night`'s text.
fn night(p: &mut Py, v: &Value) -> R<()> {
    let g = |key: &str| get_obj(v, key);
    let gov = or(g("governor"), empty_dict());
    let mc = or(
        dget(&gov, "machine", &at_module(line::NIGHT_MACHINE))?,
        empty_dict(),
    );
    let c = or(g("counts"), empty_dict());
    p.w(&format!(
        "{} (night of {}, {}): runs spread {}-{}, last call {}, new day {}.\n",
        if g("tonight").is_some_and(py::truthy) {
            "Tonight"
        } else {
            "Last night"
        },
        str_or_none(g("night")),
        str_or_none(g("zone")),
        str_or_none(g("runAt")),
        str_or_none(g("spreadEndsAt")),
        str_or_none(g("lastCallAt")),
        str_or_none(g("dayStartsAt")),
    ));
    let free = dget(&mc, "availableBytes", &at_module(line::NIGHT_FREE))?.cloned();
    let load = get_obj(&mc, "load1").cloned();
    let slots = get_obj(&gov, "slots");
    let cores = get_obj(&mc, "cores");
    let free_text = match number(free.as_ref()) {
        Some(f) => format!(
            ", {} GiB free",
            tenths(f, true).map_err(|e| Raise::at(line::NIGHT_FREE_GIB, e))?
        ),
        None => String::new(),
    };
    let load_text = match number(load.as_ref()) {
        Some(l) => format!(
            ", load {}",
            tenths(l, false).map_err(|e| Raise::at(line::NIGHT_LOAD, e))?
        ),
        None => String::new(),
    };
    p.w(&format!(
        "This computer: {} at a time ({} cores{free_text}{load_text}).\n",
        str_or_none(slots),
        str_or_none(cores)
    ));
    if let Some(h) = get_obj(&gov, "holding").filter(|h| py::truthy(h)) {
        p.w(&format!("Holding new starts: {}.\n", py::str_of(h)));
    }
    let at = at_module(line::NIGHT_COUNTS);
    let zero = Value::Int("0".into());
    let count = |key: &str| -> R<String> { Ok(py::str_of(dget(&c, key, &at)?.unwrap_or(&zero))) };
    let (queued, running, done, skipped) = (
        count("queued")?,
        count("running")?,
        count("done")?,
        count("skipped")?,
    );
    let failed = match get_obj(&c, "failed").filter(|f| py::truthy(f)) {
        Some(f) => format!(" · failed {}", py::str_of(f)),
        None => String::new(),
    };
    p.w(&format!(
        "Queued {queued} · running {running} · done {done} · skipped {skipped}{failed}\n"
    ));
    for o in py_iter(&or(g("orgs"), Value::Array(Vec::new())), line::NIGHT_ORGS)? {
        let state = dget(&o, "state", &at_module(line::NIGHT_STATE))?
            .cloned()
            .unwrap_or_else(none);
        let state = if state == super::s("done") && get_obj(&o, "wrote").is_some_and(py::truthy) {
            super::s("done (wrote its bulletin)")
        } else if state == super::s("skipped") {
            super::s("skipped (the bulletin stays as it was)")
        } else {
            state
        };
        let at = at_module(line::NIGHT_ROW);
        let due = get_obj(&o, "due");
        let name = sub(sub(&o, "org", &at)?, "name", &at)?;
        let handle = sub(sub(&o, "head", &at)?, "handle", &at)?;
        p.w(&format!(
            "  {}  {} · head: {} · {}\n",
            str_or_none(due),
            py::str_of(name),
            py::str_of(handle),
            py::str_of(&state)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
