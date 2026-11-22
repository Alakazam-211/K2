//! `k5 voice …`, as the shell CLI's `cmd_voice` answers it
//! (`cli/src/hostchat-voice.sh`): this server's voice rooms, their
//! recordings and transcripts. Each verb is one `_hostchat_call`
//! ([`self::sew`]) and `_voice_print`'s lines ([`print`]).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates. The connection gate lets `voice` alone,
//!    `voice ""` and `voice help` by, as the shell's does (the virtual
//!    desktop's guard does not).
//! 2. `voice` rides the locked `hostchat` tool, an `id` tool: with
//!    `HOOK_SOCK` set, the stale-passport or require-passport lines, exit 3.
//! 3. The words ([`parse`]): `-h` or `--help` anywhere is the usage, exit
//!    0, nothing sent (`_wants_help`); `_voice_bad`'s line
//!    (`{"error":"usage",…}`, through Python's `json.dumps`), exit 2.
//! 4. The call, then the lines, or the answer as it came with `--json`.
//!
//! Handed to the shell, before anything is printed or sent: a value flag
//! (`--room`, `--after`, `--limit`, `--recording`, `--call`, `--notice`,
//! `--require-recording`, `--start-level`) as the last word or with an
//! empty value (the shell stops on `${2:?…}` with a line naming its own
//! path: one of bash's own edges); `set rooms` with
//! a count longer than 18 digits (the shell's `test` can't read it); a
//! variable `_compute_fail` or `_compute_json` reads from the environment
//! ([`self::sew::env_shell`]); and the forms every family hands back.
//! An answer that is not UTF-8 is read with U+FFFD for each bad byte.

pub mod sew;

use self::sew::Field;
use super::{curl_port, py_iter, py_len, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing, Gate};
use crate::python;
use crate::text;
use crate::transport::encode::quote;
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};
use std::ffi::OsString;
use std::path::Path;

/// The tool's name in the shell's policy: `voice` rides `hostchat`.
pub const TOOL: &str = "hostchat";

/// The server's own voice Space: its name is K5 by design, whatever the
/// brand.
pub(crate) const SPACE: &str = "K5";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// `_voice_usage`, exit 0.
    Help,
    /// `_voice_bad "<it>"`, exit 2.
    Bad(String),
    /// The shell's (see the module's notes).
    Shell,
    /// One call, then `_voice_print <verb>`.
    Call {
        post: bool,
        path: &'static str,
        payload: String,
        verb: &'static str,
        json: bool,
    },
}

fn bad(text: impl Into<String>) -> Plan {
    Plan::Bad(text.into())
}

fn get(path: &'static str, payload: String, verb: &'static str, json: bool) -> Plan {
    Plan::Call {
        post: false,
        path,
        payload,
        verb,
        json,
    }
}

fn post(path: &'static str, fields: &[(&str, Field)], verb: &'static str, json: bool) -> Plan {
    Plan::Call {
        post: true,
        path,
        payload: sew::json(fields).unwrap_or_default(),
        verb,
        json,
    }
}

fn digits(t: &str) -> bool {
    !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit())
}

/// The words after `voice`.
pub fn parse(words: &[&str]) -> Plan {
    // `_wants_help`: -h or --help anywhere, even where a value flag would
    // take it, is the usage, and nothing is sent.
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return Plan::Help;
    }
    let Some((&sub, rest)) = words.split_first() else {
        return Plan::Help;
    };
    if sub == "help" {
        return Plan::Help;
    }
    let (mut json, mut now) = (false, false);
    let (mut call, mut notice, mut rec_req, mut rec_level) = ("", "", "", "");
    let (mut vt_room, mut vt_after, mut vt_limit, mut vt_rec) = ("", "", "", "");
    let mut pos: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let slot = match w {
            "--json" => {
                json = true;
                None
            }
            "--now" => {
                now = true;
                None
            }
            "--room" => Some(&mut vt_room),
            "--after" => Some(&mut vt_after),
            "--limit" => Some(&mut vt_limit),
            "--recording" => Some(&mut vt_rec),
            "--call" => Some(&mut call),
            "--notice" => Some(&mut notice),
            "--require-recording" => Some(&mut rec_req),
            "--start-level" => Some(&mut rec_level),
            w if w.starts_with('-') => return bad(format!("unknown flag {w}")),
            w => {
                pos.push(w);
                None
            }
        };
        match slot {
            Some(slot) => {
                // `${2:?…}`: the shell's own failure, its path in it.
                match rest.get(i + 1) {
                    Some(&v) if !v.is_empty() => *slot = v,
                    _ => return Plan::Shell,
                }
                i += 2;
            }
            None => i += 1,
        }
    }
    let n = pos.len();
    if now && !matches!(sub, "set" | "close") {
        return bad(format!(
            "--now only applies to {CLI_NAME} voice set rooms and {CLI_NAME} voice close"
        ));
    }
    if !notice.is_empty() && sub != "consent" {
        return bad(format!("--notice only applies to {CLI_NAME} voice consent"));
    }
    if (!rec_req.is_empty() || !rec_level.is_empty()) && sub != "settings" {
        return bad(format!(
            "--require-recording and --start-level only apply to {CLI_NAME} voice settings"
        ));
    }
    let any_vt = [vt_room, vt_after, vt_limit, vt_rec]
        .iter()
        .any(|v| !v.is_empty());
    if any_vt && !matches!(sub, "recordings" | "transcript" | "transcripts") {
        return bad(format!("--room, --after, --limit and --recording only apply to {CLI_NAME} voice recordings, transcript and transcripts"));
    }
    if [vt_after, vt_limit, vt_rec]
        .iter()
        .any(|v| !v.is_empty() && !digits(v))
    {
        return bad("--after, --limit and --recording are whole numbers");
    }
    let mut vt_q = String::new();
    if !vt_room.is_empty() {
        vt_q.push_str(&format!("&room={}", quote(vt_room)));
    }
    if !vt_after.is_empty() {
        vt_q.push_str(&format!("&after={vt_after}"));
    }
    if !vt_limit.is_empty() {
        vt_q.push_str(&format!("&limit={vt_limit}"));
    }
    let q_tail = vt_q.strip_prefix('&').unwrap_or(&vt_q).to_string();
    let s = |t: &str| Field::S(t.to_string());
    let b = |on: bool| Field::B(if on { "1" } else { "" }.to_string());
    match sub {
        "recordings" => {
            if n != 0 {
                return bad(format!("{CLI_NAME} voice recordings [--room <room>]"));
            }
            if !vt_rec.is_empty() {
                return bad(format!(
                    "--recording only applies to {CLI_NAME} voice transcripts reads"
                ));
            }
            get("/cli/voice/recordings", q_tail, "recordings", json)
        }
        "transcript" => {
            if n != 1 {
                return bad(format!(
                    "{CLI_NAME} voice transcript <recording-id> [--after <line-id>] [--limit N]"
                ));
            }
            if !digits(pos[0]) {
                return bad(format!(
                    "the recording id is a number ({CLI_NAME} voice recordings lists them)"
                ));
            }
            if !vt_room.is_empty() || !vt_rec.is_empty() {
                return bad(format!(
                    "{CLI_NAME} voice transcript takes --after and --limit"
                ));
            }
            get(
                "/cli/voice/transcript",
                format!("id={}{vt_q}", pos[0]),
                "transcript",
                json,
            )
        }
        "transcripts" => match pos.first().copied().unwrap_or("list") {
            "list" => {
                if n > 1 {
                    return bad(format!("{CLI_NAME} voice transcripts [--room <room>]"));
                }
                get("/cli/voice/transcripts", q_tail, "transcripts", json)
            }
            t @ ("subscribe" | "unsubscribe") => {
                if n != 3 {
                    return bad(format!(
                        "{CLI_NAME} voice transcripts {t} <room> <workspace>"
                    ));
                }
                let on = if t == "subscribe" { "1" } else { "0" };
                post(
                    "/cli/voice/transcripts/subscribe",
                    &[
                        ("room", s(pos[1])),
                        ("agent", s(pos[2])),
                        ("on", Field::B(on.into())),
                    ],
                    "subscribe",
                    json,
                )
            }
            "reads" => {
                if n != 1 {
                    return bad(format!(
                        "{CLI_NAME} voice transcripts reads [--recording <id>]"
                    ));
                }
                let mut rq = String::new();
                if !vt_rec.is_empty() {
                    rq = format!("id={vt_rec}");
                }
                if !vt_limit.is_empty() {
                    rq = if rq.is_empty() {
                        format!("limit={vt_limit}")
                    } else {
                        format!("{rq}&limit={vt_limit}")
                    };
                }
                get("/cli/voice/transcripts/reads", rq, "reads", json)
            }
            _ => bad(format!(
                "{CLI_NAME} voice transcripts [list|subscribe|unsubscribe|reads]"
            )),
        },
        "settings" => {
            if n != 0 {
                return bad(format!("{CLI_NAME} voice settings [--start-level none|moderator|member|inherit] [--require-recording on|off|inherit]"));
            }
            if rec_req.is_empty() && rec_level.is_empty() {
                return get("/cli/voice/status", String::new(), "settings", json);
            }
            let rq = match rec_req {
                "" => "",
                "on" | "true" | "yes" => "true",
                "off" | "false" | "no" => "false",
                "inherit" => "null",
                _ => return bad("--require-recording is on, off or inherit"),
            };
            let lv = match rec_level {
                "" => String::new(),
                "none" | "moderator" | "member" => format!("\"{rec_level}\""),
                "inherit" => "null".to_string(),
                _ => {
                    return bad(format!(
                        "--start-level is none, moderator, member or inherit (the {SPACE} rooms have no admin level)"
                    ))
                }
            };
            post(
                "/cli/voice/recording/settings",
                &[
                    ("required", Field::J(rq.into())),
                    ("startLevel", Field::J(lv)),
                ],
                "settings",
                json,
            )
        }
        "rooms" | "list" => {
            if n != 0 {
                return bad(format!("{CLI_NAME} voice rooms takes no arguments"));
            }
            get("/cli/voice/rooms", String::new(), "rooms", json)
        }
        "status" => {
            if n != 0 {
                return bad(format!("{CLI_NAME} voice status takes no arguments"));
            }
            get("/cli/voice/status", String::new(), "status", json)
        }
        "repair" => {
            if n != 0 {
                return bad(format!("{CLI_NAME} voice repair takes no arguments"));
            }
            Plan::Call {
                post: true,
                path: "/cli/voice/repair",
                payload: "{}".into(),
                verb: "repair",
                json,
            }
        }
        "set" => {
            if !(n == 2 && pos[0] == "rooms") {
                return bad(format!("{CLI_NAME} voice set rooms <0-10> [--now]"));
            }
            if !digits(pos[1]) {
                return bad("the room count is a number 0-10");
            }
            let count = pos[1].trim_start_matches('0');
            if count.len() > 18 {
                return Plan::Shell;
            }
            if count.parse::<u64>().unwrap_or(0) > 10 {
                return bad("the room count is 0-10");
            }
            post(
                "/cli/voice/settings",
                &[("count", Field::N(pos[1].into())), ("now", b(now))],
                "set",
                json,
            )
        }
        "rename" => {
            if n < 2 {
                return bad(format!("{CLI_NAME} voice rename <room> <name>"));
            }
            let name = pos[1..].join(" ");
            post(
                "/cli/voice/rooms/rename",
                &[("room", s(pos[0])), ("name", s(&name))],
                "rename",
                json,
            )
        }
        "close" => {
            if n != 1 {
                return bad(format!("{CLI_NAME} voice close <room> [--now]"));
            }
            post(
                "/cli/voice/rooms/close",
                &[("room", s(pos[0])), ("now", b(now))],
                "close",
                json,
            )
        }
        "join" => {
            if n != 1 {
                return bad(format!("{CLI_NAME} voice join <room>"));
            }
            post("/cli/voice/join", &[("room", s(pos[0]))], "join", json)
        }
        "leave" => {
            if n != 0 {
                return bad(format!("{CLI_NAME} voice leave [--call <callId>]"));
            }
            post(
                "/cli/voice/session/leave",
                &[("callId", s(call))],
                "leave",
                json,
            )
        }
        "consent" => {
            let cu = format!("{CLI_NAME} voice consent <recording-id> yes --notice <version> | {CLI_NAME} voice consent <recording-id> no");
            if n != 2 {
                return bad(cu);
            }
            if !digits(pos[0]) {
                return bad(format!(
                    "the recording id is a number (from the notice): {cu}"
                ));
            }
            let cb = match pos[1] {
                "yes" => {
                    if notice.is_empty() {
                        return bad(
                            "yes needs --notice <version>, the notice_version you were shown",
                        );
                    }
                    "1"
                }
                "no" => "0",
                _ => return bad(format!("say yes or no: {cu}")),
            };
            if !notice.is_empty() && !digits(notice) {
                return bad("--notice is the notice's version number");
            }
            post(
                "/cli/voice/recording/consent",
                &[
                    ("recording_id", s(pos[0])),
                    ("consent", Field::B(cb.into())),
                    ("notice_version", Field::N(notice.into())),
                ],
                "consent",
                json,
            )
        }
        "kick" | "mute" | "unmute" | "deafen" | "undeafen" => {
            if n != 2 {
                return bad(format!("{CLI_NAME} voice {sub} <room> <person>"));
            }
            post(
                "/cli/voice/moderate",
                &[
                    ("room", s(pos[0])),
                    ("person", s(pos[1])),
                    ("action", s(sub)),
                ],
                "moderate",
                json,
            )
        }
        _ => bad(format!("unknown command {sub}")),
    }
}

/// `_voice_bad`'s line: `json.dumps({"error": "usage", "hint": …},
/// separators=(",", ":"))`, so a quote, a backslash or a character outside
/// ASCII in the words is escaped as Python escapes it.
pub fn bad_line(text: &str) -> String {
    let line = Value::Object(vec![
        (Text::from("error"), super::s("usage")),
        (
            Text::from("hint"),
            super::s(&format!("{text} ({CLI_NAME} voice --help)")),
        ),
    ]);
    let options = json::Options {
        compact: true,
        ..json::Options::default()
    };
    json::to_string(&line, &options).expect("ASCII JSON of two strings")
}

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["voice"];
/// The binary answers it ([`run`] hands some lines back to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 voice ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(n, _)| n)) {
        return None;
    }
    if sew::env_shell(std::env::vars_os().map(|(n, _)| n)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let plan = parse(&words);
    if plan == Plan::Shell || !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    // The shell's gate lets `voice`, `voice ""` and `voice help …` by.
    let skip = matches!(words.first(), None | Some(&("" | "help")));
    if !skip
        && prelude::gate(&settings).is_none()
        && settings.port_text.as_deref().and_then(curl_port).is_none()
    {
        return None;
    }
    Some(run_with(args, plan, skip, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    plan: Plan,
    skip: bool,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    match prelude::gate(&settings) {
        Some(g @ Gate::VirtualDesktop) => return g.stop(),
        Some(g @ Gate::CannotConnect) if !skip => return g.stop(),
        _ => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return stop;
    }
    let (post, path, payload, verb, json) = match plan {
        Plan::Help => {
            return match out::stdout(text::help("voice").unwrap_or_default()) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Plan::Bad(text) => {
            out::stderr_line(&bad_line(&text));
            return Exit::Usage;
        }
        Plan::Shell => unreachable!("run hands it to the shell"),
        Plan::Call {
            post,
            path,
            payload,
            verb,
            json,
        } => (post, path, payload, verb, json),
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match sew::call(&mut client, &mut closing, post, path, &payload, false) {
        Err(exit) => exit,
        Ok(body) if json => {
            let mut text = String::from_utf8_lossy(&body).into_owned();
            text.push('\n');
            match out::stdout(&text) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Ok(body) => print(&String::from_utf8_lossy(&body), verb).print(),
    };
    closing.flush(&mut client, exit);
    exit
}

/// The lines of `_voice_print`'s Python, as `python3 -c` numbers them.
pub mod line {
    pub const LOAD: u32 = 3;
    pub const H: u32 = 5;
    pub const H_STATE: u32 = 7;
}

/// A Python reader's state: what it printed.
#[derive(Default)]
struct Py {
    out: String,
}

impl Py {
    fn line(&mut self, t: &str) {
        self.out.push_str(t);
        self.out.push('\n');
    }
}

type R<T> = Result<T, Raise>;

/// `v.get(k)` (`None` as null), or the `AttributeError` at `line`.
fn g(v: &Value, k: &str, line: u32) -> R<Value> {
    match v {
        Value::Object(pairs) => Ok(super::get(pairs, k).cloned().unwrap_or(Value::Null)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `v[k]` where the key is there for sure.
fn idx(v: &Value, k: &str) -> Value {
    g(v, k, 0).unwrap_or(Value::Null)
}

fn t(v: &Value) -> bool {
    py::truthy(v)
}

/// `a or b`.
fn or(a: Value, b: Value) -> Value {
    if t(&a) {
        a
    } else {
        b
    }
}

fn st(text: &str) -> Value {
    Value::Str(json::Text::from(text))
}

fn sv(v: &Value) -> String {
    py::str_of(v)
}

fn empty_dict() -> Value {
    Value::Object(Vec::new())
}

fn empty_list() -> Value {
    Value::Array(Vec::new())
}

/// Python's `==` for JSON values: numbers by value (a bool is 0 or 1).
fn eq(a: &Value, b: &Value) -> bool {
    fn num(v: &Value) -> Option<f64> {
        match v {
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Int(d) => d.parse().ok(),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, v)| y.iter().any(|(k2, v2)| k == k2 && eq(v, v2)))
        }
        _ => match (num(a), num(b)) {
            (Some(x), Some(y)) => x == y,
            (None, None) => a == b,
            _ => false,
        },
    }
}

fn is(v: &Value, text: &str) -> bool {
    matches!(v, Value::Str(s) if s.as_str() == Some(text))
}

/// `"<text>" + v`: the `TypeError` at `line` unless `v` is a string.
fn cat(head: &str, v: &Value, line: u32) -> R<String> {
    match v {
        Value::Str(_) => Ok(format!("{head}{}", sv(v))),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: can only concatenate str (not \"{}\") to str",
                py::type_name(other)
            ),
        )),
    }
}

/// `", ".join(v)`: strings only.
fn join(v: &Value, line: u32) -> R<String> {
    let items =
        py_iter(v, line).map_err(|_| Raise::at(line, "TypeError: can only join an iterable"))?;
    join_items(&items, line)
}

fn join_items(items: &[Value], line: u32) -> R<String> {
    let mut out = Vec::new();
    for (i, x) in items.iter().enumerate() {
        match x {
            Value::Str(_) => out.push(sv(x)),
            other => {
                return Err(Raise::at(
                    line,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(other)
                    ),
                ))
            }
        }
    }
    Ok(out.join(", "))
}

fn left(v: &Value, w: usize) -> String {
    super::ljust(&sv(v), w)
}

/// `int(v)`, Python's way, or the exception at `line`.
pub(super) fn int_of(v: &Value, line: u32) -> R<i128> {
    let err = |e: String| Raise::at(line, e);
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Int(d) => d
            .parse()
            .map_err(|_| err("OverflowError: int too large".into())),
        Value::Float(f) if f.is_nan() => Err(err(
            "ValueError: cannot convert float NaN to integer".into(),
        )),
        Value::Float(f) if f.is_infinite() => Err(err(
            "OverflowError: cannot convert float infinity to integer".into(),
        )),
        Value::Float(f) => Ok(f.trunc() as i128),
        Value::Str(_) => {
            let text = sv(v);
            let trimmed = text.trim_matches(super::py_space);
            let (neg, body) = match trimmed.as_bytes().first() {
                Some(b'-') => (true, &trimmed[1..]),
                Some(b'+') => (false, &trimmed[1..]),
                _ => (false, trimmed),
            };
            let ok = !body.is_empty()
                && !body.starts_with('_')
                && !body.ends_with('_')
                && !body.contains("__")
                && body.chars().all(|c| c.is_ascii_digit() || c == '_');
            if !ok {
                return Err(err(format!(
                    "ValueError: invalid literal for int() with base 10: {}",
                    py::repr(v)
                )));
            }
            let n: i128 = body
                .replace('_', "")
                .parse()
                .map_err(|_| err("OverflowError: int too large".into()))?;
            Ok(if neg { -n } else { n })
        }
        other => Err(err(format!(
            "TypeError: int() argument must be a string, a bytes-like object or a real number, not '{}'",
            py::type_name(other)
        ))),
    }
}

/// `a // b` and `a % b`, Python's way (floor).
fn floor_div(a: i128, b: i128) -> i128 {
    let q = a / b;
    if (a % b != 0) && ((a < 0) != (b < 0)) {
        q - 1
    } else {
        q
    }
}

fn modulo(a: i128, b: i128) -> i128 {
    a - b * floor_div(a, b)
}

/// `"%02d" % n`: a negative number is two characters already.
fn d2(n: i128) -> String {
    if n < 0 {
        n.to_string()
    } else {
        format!("{n:02}")
    }
}

/// `_voice_print <verb>` on the body: its lines, or where its Python
/// raised.
pub fn print(raw: &str, verb: &str) -> Shown {
    let mut p = Py::default();
    let r = print_into(&mut p, raw, verb);
    match r {
        Ok(()) => Shown::out(p.out),
        Err(e) => super::raised(p.out, String::new(), &e),
    }
}

fn hosting_line(h: &Value, from: u32) -> R<String> {
    let inside = |r: Raise| Raise::inside(from, line::H_STATE, "hosting_line", r.exception);
    let state = g(h, "state", line::H_STATE).map_err(inside)?;
    if is(&state, "ok") {
        return Ok("on".into());
    }
    let mut s = format!("{}: {}", sv(&state), sv(&or(idx(h, "line"), st(""))));
    if t(&idx(h, "detail")) {
        s.push_str(&format!(" ({})", sv(&idx(h, "detail"))));
    }
    Ok(s)
}

fn print_into(p: &mut Py, raw: &str, verb: &str) -> R<()> {
    let v = json::parse(raw).map_err(|_| {
        Raise::at(
            line::LOAD,
            "json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)",
        )
    })?;
    let h = or(g(&v, "hosting", line::H)?, empty_dict());
    match verb {
        "rooms" => {
            let hl = hosting_line(&h, 14)?;
            p.line(&format!(
                "Voice rooms on {}: {hl}",
                sv(&or(idx(&v, "server"), st("this server")))
            ));
            let rooms = or(idx(&v, "rooms"), empty_list());
            if !t(&rooms) {
                p.line("  no rooms open");
            }
            for r in py_iter(&rooms, 18)? {
                let occ = or(g(&r, "occupants", 19)?, empty_list());
                let tag = if t(&idx(&r, "closing")) {
                    "  closing".to_string()
                } else if t(&idx(&r, "unavailable")) {
                    format!("  unavailable: {}", sv(&idx(&r, "unavailable")))
                } else {
                    String::new()
                };
                p.line(&format!(
                    "  {} {} {}/{}{tag}",
                    left(&idx(&r, "slug"), 8),
                    left(&idx(&r, "name"), 24),
                    py_len(&occ, 25)?,
                    sv(&idx(&r, "limit"))
                ));
                for o in py_iter(&occ, 26)? {
                    let mut flags = Vec::new();
                    if t(&g(&o, "muted", 27)?) {
                        flags.push("muted");
                    }
                    if t(&g(&o, "deafened", 27)?) {
                        flags.push("deafened");
                    }
                    let tail = if flags.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", flags.join(", "))
                    };
                    p.line(&format!("      {}{tail}", sv(&idx(&o, "user"))));
                }
            }
            let me = or(idx(&v, "me"), empty_dict());
            if t(&g(&me, "inCall", 30)?) {
                let wher = if t(&idx(&me, "here")) {
                    "this login"
                } else {
                    "another device or session"
                };
                let room = or(idx(&me, "room"), empty_dict());
                let name = or(g(&room, "name", 32)?, idx(&me, "channelId"));
                p.line(&format!(
                    "you: in {} from {wher} (if that app was closed, this can lag up to {} s)",
                    sv(&name),
                    sv(&idx(&me, "presenceLagSecs"))
                ));
            }
        }
        "status" => status(p, &v, &h)?,
        "repair" => repair(p, &v)?,
        "set" | "close" => {
            p.line(&format!("rooms: {}", sv(&idx(&v, "count"))));
            if t(&idx(&v, "closing")) {
                let names = join(&idx(&v, "closing"), 100)?;
                let when = if t(&idx(&v, "closeNowAt")) {
                    " (calls end in 10 s)"
                } else {
                    " (when empty)"
                };
                p.line(&format!("closing: {names}{when}"));
            }
        }
        "rename" => p.line(&format!(
            "{} is now {}",
            sv(&idx(&v, "room")),
            sv(&idx(&v, "name"))
        )),
        "join" => {
            let r = or(idx(&v, "room"), empty_dict());
            let name = g(&r, "name", 105)?;
            p.line(&format!("joined {} (call {}). The CLI has no audio: a voice client connects with the answer (--json).", sv(&name), sv(&idx(&v, "callId"))));
        }
        "leave" => {
            let w = or(idx(&v, "why"), st(""));
            if t(&idx(&v, "already")) {
                let said = match &w {
                    Value::Array(_) | Value::Object(_) => {
                        return Err(Raise::at(
                            113,
                            format!("TypeError: unhashable type: '{}'", py::type_name(&w)),
                        ))
                    }
                    Value::Str(s) => match s.as_str().unwrap_or("") {
                        "already_left" => "you left it before".to_string(),
                        "moved" => "you joined another room from here".to_string(),
                        "joined_elsewhere" => {
                            "you joined from another device or session; that call goes on"
                                .to_string()
                        }
                        "left_elsewhere" => "you left from another device or session".to_string(),
                        "sew_dropped" => "the voice server had already dropped you".to_string(),
                        "in_another_room" => {
                            "you are in another room now; that call goes on".to_string()
                        }
                        _ => sv(&w),
                    },
                    _ => sv(&w),
                };
                p.line(&format!(
                    "nothing to leave: that call already ended ({said}) [{}]",
                    sv(&w)
                ));
            } else {
                p.line("left");
            }
        }
        "settings" => {
            let rec = match idx(&v, "recording") {
                r @ Value::Object(_) => r,
                _ => v.clone(),
            };
            let fl = or(idx(&rec, "flags"), empty_dict());
            if let Value::Object(pairs) = &v {
                if super::get(pairs, "changed").is_some() {
                    let said = if t(&idx(&v, "changed")) {
                        "saved"
                    } else {
                        "nothing changed (already so)"
                    };
                    p.line(&format!("{SPACE} rooms recording: {said}"));
                }
            }
            if !t(&g(&fl, "appRecording", 119)?) {
                p.line(&format!("This server's chat can't record in rooms yet (it needs Sew 0.9.0: {CLI_NAME} hostchat upgrade)."));
            }
            let req = idx(&rec, "required");
            p.line(&format!(
                "{SPACE} rooms Space:     {}",
                sv(&or(idx(&rec, "spaceId"), st("not made yet")))
            ));
            p.line(&format!(
                "start level:        {}",
                sv(&or(
                    idx(&rec, "startLevel"),
                    st("inherit (the server's rule)")
                ))
            ));
            let rq = match &req {
                Value::Null => "inherit (the server's rule)",
                r if t(r) => "on",
                _ => "off",
            };
            p.line(&format!("require recording:  {rq}"));
            if t(&idx(&rec, "locked")) {
                p.line(&format!("LOCKED: no recording in the {SPACE} rooms (CUI/FCI); only the owner lifts it, in person"));
            }
            if t(&idx(&rec, "agentReaders")) {
                p.line(&format!("transcripts:        agents on this server may read them (each person's notice says so; {CLI_NAME} voice transcripts)"));
            }
        }
        "recordings" => {
            let items = or(idx(&v, "recordings"), empty_list());
            if !t(&items) {
                p.line("no recordings in this server's voice rooms");
            }
            for r in py_iter(&items, 133)? {
                let ready = if t(&g(&r, "transcriptReady", 134)?) {
                    "transcript ready".to_string()
                } else {
                    format!(
                        "transcript {}",
                        sv(&or(idx(&r, "transcriptState"), st("not ready")))
                    )
                };
                p.line(&format!(
                    "  {} {} {} {}  {ready}",
                    left(&idx(&r, "id"), 20),
                    left(&or(idx(&r, "room"), idx(&r, "channelId")), 10),
                    left(&idx(&r, "startedAt"), 20),
                    sv(&idx(&r, "trigger"))
                ));
            }
            if t(&idx(&v, "next")) {
                p.line(&format!(
                    "more: {CLI_NAME} voice recordings --after {}",
                    sv(&idx(&v, "next"))
                ));
            }
            if t(&items) && t(&idx(&v, "canRead")) {
                p.line(&format!("read one: {CLI_NAME} voice transcript <id>"));
            }
        }
        "transcript" => {
            let r = or(idx(&v, "recording"), empty_dict());
            let id = g(&r, "id", 142)?;
            let ready = if t(&idx(&r, "transcriptReady")) {
                "transcript ready"
            } else {
                "transcript still being written"
            };
            p.line(&format!(
                "Recording {} in {}, started {} ({ready})",
                sv(&id),
                sv(&or(idx(&r, "roomName"), idx(&r, "room"))),
                sv(&idx(&r, "startedAt"))
            ));
            let lines = or(idx(&v, "lines"), empty_list());
            if !t(&lines) {
                p.line("  (no lines on this page)");
            }
            for tl in py_iter(&lines, 146)? {
                let ms = floor_div(
                    int_of(&or(g(&tl, "startMs", 147)?, Value::Int("0".into())), 147)?,
                    1000,
                );
                p.line(&format!(
                    "[{}:{}] {}: {}",
                    d2(floor_div(ms, 60)),
                    d2(modulo(ms, 60)),
                    sv(&idx(&tl, "speaker")),
                    sv(&idx(&tl, "text"))
                ));
            }
            if t(&idx(&v, "next")) {
                p.line(&format!(
                    "more: {CLI_NAME} voice transcript {} --after {}",
                    sv(&id),
                    sv(&idx(&v, "next"))
                ));
            }
            p.line("(this read is logged)");
        }
        "transcripts" | "subscribe" => {
            if verb == "subscribe" {
                let what = if t(&idx(&v, "on")) {
                    "now gets this room's transcripts"
                } else {
                    "no longer gets this room's transcripts"
                };
                let already = if t(&idx(&v, "changed")) {
                    ""
                } else {
                    " (already so)"
                };
                p.line(&format!("{} {what}{already}", sv(&idx(&v, "agent"))));
            }
            let rd = or(idx(&v, "readers"), empty_list());
            let readers = if t(&rd) {
                join(&rd, 156)?
            } else {
                "none (Read call transcripts is off for every agent)".to_string()
            };
            p.line(&format!("agents that may read transcripts: {readers}"));
            for r in py_iter(&or(idx(&v, "rooms"), empty_list()), 157)? {
                let room = g(&r, "room", 158)?;
                let to = join(&or(idx(&r, "sendTo"), empty_list()), 158)?;
                p.line(&format!(
                    "  {} {} sends to: {}",
                    left(&room, 8),
                    left(&idx(&r, "name"), 24),
                    if to.is_empty() {
                        "nobody".to_string()
                    } else {
                        to
                    }
                ));
            }
        }
        "reads" => {
            let rows = or(idx(&v, "reads"), empty_list());
            if !t(&rows) {
                p.line("no transcript reads logged");
            }
            for x in py_iter(&rows, 163)? {
                let at = g(&x, "at", 164)?;
                p.line(&format!(
                    "  {}  {} recording {}, {} line(s)",
                    sv(&at),
                    left(&idx(&x, "reader"), 28),
                    sv(&idx(&x, "recordingId")),
                    sv(&idx(&x, "lines"))
                ));
            }
        }
        "consent" => {
            let id = sv(&idx(&v, "recordingId"));
            if t(&idx(&v, "consent")) {
                p.line(&format!("recording {id}: you said yes; it records you"));
            } else {
                p.line(&format!("recording {id}: you said no; it does not record you (a yes you gave before is withdrawn)"));
            }
        }
        _ => {
            let room = or(idx(&v, "room"), empty_dict());
            let name = g(&room, "name", 171)?;
            p.line(&format!(
                "{} {} in {}",
                sv(&idx(&v, "action")),
                sv(&idx(&v, "person")),
                sv(&name)
            ));
        }
    }
    Ok(())
}

/// `voice status`'s lines (the Python's 34-70).
fn status(p: &mut Py, v: &Value, h: &Value) -> R<()> {
    let hl = hosting_line(h, 34)?;
    p.line(&format!("hosting:    {hl}"));
    let mut tier = idx(v, "tier");
    if let Value::Object(_) = tier {
        let reason = if t(&idx(&tier, "reason")) {
            cat(": ", &idx(&tier, "reason"), 37)?
        } else {
            String::new()
        };
        tier = st(&format!(
            "{}, {} reading{reason}",
            sv(&idx(&tier, "state")),
            sv(&idx(&tier, "reading"))
        ));
    }
    p.line(&format!(
        "rooms:      {} (plan: {})",
        sv(&idx(v, "count")),
        sv(&tier)
    ));
    let sp = or(idx(v, "space"), empty_dict());
    let sh = or(idx(v, "spaceHealth"), empty_dict());
    let mut state = g(&sh, "state", 41)?;
    if !t(&state) {
        state = or(g(&sp, "state", 41)?, st("not made yet"));
    }
    if is(&state, "none") {
        state = st("not made yet");
    }
    let space_id = if t(&g(&sp, "spaceId", 44)?) {
        cat(" ", &idx(&sp, "spaceId"), 44)?
    } else {
        String::new()
    };
    p.line(&format!("{SPACE} Space: {}{space_id}", sv(&state)));
    let sh_state = idx(&sh, "state");
    if ["checking", "suspect", "blocked"]
        .iter()
        .any(|x| is(&sh_state, x))
    {
        p.line(&format!("  why:      {}", sv(&idx(&sh, "line"))));
        p.line(&format!("  fix:      {}", sv(&idx(&sh, "fix"))));
    } else if t(&idx(&sp, "why")) {
        p.line(&format!("  why:      {}", sv(&idx(&sp, "why"))));
    }
    let lc = or(idx(&sp, "lastCheck"), empty_dict());
    let outcome = g(&lc, "outcome", 51)?;
    if is(&outcome, "remade") || is(&outcome, "relinked") {
        let what = if is(&outcome, "remade") {
            "Sew made it again".to_string()
        } else {
            format!("{PRODUCT_NAME} re-linked a stale id")
        };
        p.line(&format!(
            "  last check: {what} (was {}, now {})",
            sv(&idx(&lc, "from")),
            sv(&idx(&lc, "to"))
        ));
    }
    for r in py_iter(&or(idx(v, "rooms"), empty_list()), 53)? {
        let slug = g(&r, "slug", 54)?;
        let name = or(or(idx(&r, "wantName"), idx(&r, "name")), st(""));
        let now = if t(&idx(&r, "closeNow")) {
            "  (close now)"
        } else {
            ""
        };
        p.line(&format!(
            "  {} {} {}{now}",
            left(&slug, 8),
            left(&name, 24),
            sv(&idx(&r, "state"))
        ));
    }
    let m = or(idx(v, "members"), empty_list());
    p.line(&format!("members:    {}", py_len(&m, 56)?));
    for x in py_iter(&m, 57)? {
        let state = g(&x, "state", 58)?;
        let want = idx(&x, "wantLevel");
        if !is(&state, "added") || (t(&want) && !eq(&want, &idx(&x, "level"))) {
            let why = if t(&idx(&x, "why")) {
                cat(" \u{2014} ", &idx(&x, "why"), 59)?
            } else {
                String::new()
            };
            p.line(&format!(
                "  {}: {}{why}",
                sv(&idx(&x, "externalId")),
                sv(&state)
            ));
        }
    }
    holds(p, v, 60)?;
    if t(&idx(v, "carriedAgents")) {
        p.line(&format!(
            "agents carried to k5-admin: {}",
            join(&idx(v, "carriedAgents"), 63)?
        ));
    }
    let stale = or(idx(v, "staleSpaces"), empty_list());
    if t(&stale) {
        let n = py_len(&stale, 66)?;
        let mut slugs = Vec::new();
        for s in py_iter(&stale, 66)? {
            let slug =
                g(&s, "slug", 66).map_err(|r| Raise::inside(66, 66, "<genexpr>", r.exception))?;
            slugs.push(or(slug, st("?")));
        }
        p.line(&format!("set aside:  {n} agent Space(s) from an earlier Sew install ({}): {CLI_NAME} voice repair says how to make them again", join_items(&slugs, 66)?));
    }
    let ln = or(idx(v, "lane"), empty_dict());
    let ln_state = g(&ln, "state", 68)?;
    let reason = if t(&idx(&ln, "reason")) {
        cat(" \u{2014} ", &idx(&ln, "reason"), 68)?
    } else {
        String::new()
    };
    p.line(&format!("voice lane: {}{reason}", sv(&ln_state)));
    let fd = or(idx(v, "feed"), empty_dict());
    let fd_state = g(&fd, "state", 70)?;
    let last = if t(&idx(&fd, "lastError")) {
        cat(" \u{2014} ", &idx(&fd, "lastError"), 70)?
    } else {
        String::new()
    };
    p.line(&format!("feed:       {}{last}", sv(&fd_state)));
    Ok(())
}

/// The `hold:` lines; their loop is at `line`.
fn holds(p: &mut Py, v: &Value, line: u32) -> R<()> {
    for x in py_iter(&or(idx(v, "holds"), empty_list()), line)? {
        let op = g(&x, "op", line + 1)?;
        let error = if t(&idx(&x, "error")) {
            cat(" \u{2014} ", &idx(&x, "error"), line + 1)?
        } else {
            String::new()
        };
        p.line(&format!(
            "hold:       {} {} {}{error}",
            sv(&op),
            sv(&idx(&x, "subject")),
            sv(&idx(&x, "state"))
        ));
    }
    Ok(())
}

/// `voice repair`'s lines (the Python's 72-96).
fn repair(p: &mut Py, v: &Value) -> R<()> {
    p.line(&format!("{SPACE} Space:   {}", sv(&idx(v, "space"))));
    for r in py_iter(&or(idx(v, "rooms"), empty_list()), 73)? {
        let room = g(&r, "room", 74)?;
        p.line(&format!(
            "  {}: re-linked to channel {} (was {})",
            sv(&room),
            sv(&idx(&r, "to")),
            sv(&or(idx(&r, "from"), st("none")))
        ));
    }
    let m = or(idx(v, "members"), empty_dict());
    let added = g(&m, "added", 76)?;
    p.line(&format!(
        "members:    {} in the Space, {} still being added",
        sv(&added),
        sv(&idx(&m, "pending"))
    ));
    for x in py_iter(&or(idx(&m, "held"), empty_list()), 77)? {
        let id = g(&x, "externalId", 78)?;
        p.line(&format!(
            "  held:     {} \u{2014} {}",
            sv(&id),
            sv(&idx(&x, "why"))
        ));
    }
    let fixed = or(idx(v, "fixed"), empty_list());
    for f in py_iter(&fixed, 80)? {
        p.line(&format!("fixed:      {}", sv(&f)));
    }
    if !t(&fixed) && t(&idx(v, "settled")) {
        p.line("fixed:      nothing needed fixing");
    }
    if t(&idx(v, "notRestored")) {
        p.line(&format!("note:       {}", sv(&idx(v, "notRestored"))));
    }
    let stale = or(idx(v, "staleSpaces"), empty_list());
    if t(&stale) {
        p.line(&format!(
            "set aside:  {} agent Space(s) from an earlier Sew install:",
            py_len(&stale, 88)?
        ));
        for s in py_iter(&stale, 89)? {
            let access = or(g(&s, "access", 90)?, empty_list());
            let mut who = Vec::new();
            for a in py_iter(&access, 90)? {
                let ws = g(&a, "workspace", 90)
                    .map_err(|r| Raise::inside(90, 90, "<genexpr>", r.exception))?;
                who.push(st(&format!(
                    "{} (level {})",
                    sv(&or(ws, idx(&a, "projectId"))),
                    sv(&idx(&a, "level"))
                )));
            }
            let who = join_items(&who, 90)?;
            p.line(&format!(
                "  {} (old id {}): {}",
                sv(&idx(&s, "slug")),
                sv(&idx(&s, "spaceId")),
                if who.is_empty() {
                    "no agents".to_string()
                } else {
                    who
                }
            ));
        }
        p.line(&format!("  how:      {}", sv(&idx(v, "staleHint"))));
    }
    holds(p, v, 93)?;
    if !t(&idx(v, "settled")) {
        p.line(&format!(
            "still working: {PRODUCT_NAME} keeps at it; {CLI_NAME} voice status shows progress"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
