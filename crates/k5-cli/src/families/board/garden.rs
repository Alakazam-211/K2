//! `k5 board <verb> …` on a Garden board: `cmd_board` past its `--file`
//! check (`cli/src/board.sh`).
//!
//! 1. The words ([`parse`], the shell's first Python): the verb and its
//!    aliases, the flags (`--k v`, `--k=v`, `--k` alone is true), the
//!    body for `POST /cli/zen/data/board`, and `--out`. A word the verb
//!    doesn't take is `Error [usage]: … (k2 board --help)`, exit 2.
//! 2. `_compute_send POST /cli/zen/data/board <body>` ([`super::send`]).
//! 3. Not 200: `_board_fail` ([`fail`]): the answer (`--json`: as it came)
//!    or its error, and the exit its code maps to.
//! 4. 200 with no `--board`: the one-board warning on stderr.
//! 5. `--json`: the answer with `kind` set to `garden` when it has none,
//!    as `json.dumps(v, ensure_ascii=False)`. Else [`show`]: what the
//!    answer holds, by its keys (`export` writes the file).

use super::Sent;
use crate::exit::Exit;
use crate::families::{py_d, py_len, raised, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::py as pyfmt;
use crate::out::pylib::json as pyjson;
use crate::out::pylib::ops::{contains, get_in, index, join, or, s_of};

/// The route.
pub const ENDPOINT: &str = "/cli/zen/data/board";

/// The verbs the daemon takes, after the aliases.
const KNOWN: [&str; 21] = [
    "boards",
    "list",
    "read",
    "search",
    "log",
    "fields",
    "templates",
    "add",
    "frame",
    "update",
    "delete",
    "connect",
    "template",
    "react",
    "vote",
    "comment",
    "meta",
    "apply",
    "board-new",
    "board-rename",
    "export",
];

/// The parser's lines that raise (`int(n)` of `--limit`).
const L_LIMIT: u32 = 50;

/// What the words ask for.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask {
    /// The request's body.
    pub body: Value,
    /// `--out <path>`, as `$(…)` holds it (trailing newlines gone).
    pub out: String,
}

/// Why the words stop before a request: a usage line, or the parser's
/// own exception. Both end the command with exit 2.
#[derive(Debug, Clone, PartialEq)]
pub enum Stop {
    Usage(String),
    Raised(Raise),
}

impl Stop {
    pub fn print(&self) -> Exit {
        let text = match self {
            Stop::Usage(m) => format!(
                concat!(
                    "Error [usage]: {} (",
                    k2_client::cli_name!(),
                    " board --help)\n"
                ),
                m
            ),
            Stop::Raised(r) => r.text(),
        };
        let _ = crate::out::write_flushed(&mut std::io::stderr().lock(), text.as_bytes());
        Exit::Usage
    }
}

/// A flag's value: a word, or `True` (no word after it).
#[derive(Debug, Clone, PartialEq)]
enum Flag {
    Word(String),
    True,
}

impl Flag {
    fn value(&self) -> Value {
        match self {
            Flag::Word(w) => Value::Str(Text::from(w.as_str())),
            Flag::True => Value::Bool(true),
        }
    }

    fn truthy(&self) -> bool {
        match self {
            Flag::Word(w) => !w.is_empty(),
            Flag::True => true,
        }
    }
}

/// A dict of flags in Python's order: a key set again keeps its place.
#[derive(Debug, Default)]
struct Flags(Vec<(String, Flag)>);

impl Flags {
    fn set(&mut self, k: String, v: Flag) {
        match self.0.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => self.0.push((k, v)),
        }
    }

    fn pop(&mut self, k: &str) -> Option<Flag> {
        let at = self.0.iter().position(|(key, _)| key == k)?;
        Some(self.0.remove(at).1)
    }

    fn drain(&mut self) -> Value {
        let pairs = self
            .0
            .drain(..)
            .map(|(k, v)| (Text::from(k), v.value()))
            .collect();
        Value::Object(pairs)
    }
}

fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// The shell's first Python on the words after `board` (the verb first;
/// there is one, and it is not empty). `stdin`: what `apply -` reads.
pub fn parse(argv: &[&str], stdin: impl FnOnce() -> Vec<u8>) -> Result<Ask, Stop> {
    let die = |m: String| Err(Stop::Usage(m));
    let mut pos: Vec<&str> = Vec::new();
    let mut flags = Flags::default();
    let mut i = 1;
    while i < argv.len() {
        let x = argv[i];
        if x == "--json" {
            i += 1;
            continue;
        }
        if x.starts_with("--") && x.chars().count() > 2 {
            let (k, v) = if let Some((k, v)) = x[2..].split_once('=') {
                (k.to_string(), Flag::Word(v.to_string()))
            } else if i + 1 < argv.len() && !argv[i + 1].starts_with("--") {
                i += 1;
                (x[2..].to_string(), Flag::Word(argv[i].to_string()))
            } else {
                (x[2..].to_string(), Flag::True)
            };
            flags.set(k, v);
        } else {
            pos.push(x);
        }
        i += 1;
    }
    let cmd = match argv[0] {
        "ls" => "list",
        "rm" | "del" => "delete",
        "set" => "update",
        "new" => "board-new",
        "rename" => "board-rename",
        other => other,
    };
    if !KNOWN.contains(&cmd) {
        return die(format!("unknown command {cmd}"));
    }
    let mut b: Vec<(Text, Value)> = vec![(Text::from("cmd"), s(cmd))];
    let put = |b: &mut Vec<(Text, Value)>, k: &str, v: Value| {
        let k = Text::from(k);
        match b.iter_mut().find(|(key, _)| *key == k) {
            Some(slot) => slot.1 = v,
            None => b.push((k, v)),
        }
    };
    match flags.pop("board") {
        Some(Flag::True) => return die("--board takes a board id or name".into()),
        Some(Flag::Word(w)) => put(&mut b, "board", s(&w)),
        None => {}
    }
    let out = match flags.pop("out") {
        Some(Flag::Word(w)) => w,
        _ => String::new(),
    };
    let num = |flags: &mut Flags, k: &str| -> Result<Option<f64>, Stop> {
        match flags.pop(k) {
            None => Ok(None),
            Some(Flag::True) => Ok(Some(1.0)),
            Some(Flag::Word(w)) => crate::out::pylib::ops::float(&w)
                .map(Some)
                .ok_or_else(|| Stop::Usage(format!("--{k} takes a number"))),
        }
    };
    let take = |flags: &mut Flags, k: &str| -> Result<Option<String>, Stop> {
        match flags.pop(k) {
            Some(Flag::True) => Err(Stop::Usage(format!("--{k} takes a value"))),
            Some(Flag::Word(w)) => Ok(Some(w)),
            None => Ok(None),
        }
    };
    let full = |flags: &mut Flags| flags.pop("full").is_some_and(|f| f.truthy());
    match cmd {
        "list" => {
            if let Some(t) = take(&mut flags, "type")?.filter(|t| !t.is_empty()) {
                put(&mut b, "type", s(&t));
            }
            if full(&mut flags) {
                put(&mut b, "full", Value::Bool(true));
            }
        }
        "read" => {
            let Some(id) = pos.first() else {
                return die("read needs an object id".into());
            };
            put(&mut b, "id", s(id));
            if full(&mut flags) {
                put(&mut b, "full", Value::Bool(true));
            }
        }
        "search" => {
            if pos.is_empty() {
                return die("search needs text".into());
            }
            put(&mut b, "text", s(&pos.join(" ")));
        }
        "log" => {
            if let Some(n) = num(&mut flags, "limit")? {
                let digits = pyfmt::percent_d(&Value::Float(n))
                    .map_err(|e| Stop::Raised(Raise::at(L_LIMIT, e)))?;
                put(&mut b, "limit", Value::Int(digits));
            }
        }
        "fields" => {
            if let Some(t) = pos.first() {
                put(&mut b, "type", s(t));
            }
        }
        "boards" => {
            if flags.pop("all").is_some_and(|f| f.truthy()) {
                put(&mut b, "all", Value::Bool(true));
            }
        }
        "add" | "frame" => {
            if cmd == "add" {
                put(&mut b, "type", s(pos.first().copied().unwrap_or("sticky")));
            }
            put(&mut b, "fields", flags.drain());
        }
        "update" => {
            let Some(id) = pos.first() else {
                return die("update needs an object id".into());
            };
            put(&mut b, "id", s(id));
            let fields = flags.drain();
            let empty = matches!(&fields, Value::Object(p) if p.is_empty());
            put(&mut b, "fields", fields);
            if empty {
                return die("update needs at least one --<field> <value>".into());
            }
        }
        "delete" => {
            let Some(id) = pos.first() else {
                return die("delete needs an object id".into());
            };
            put(&mut b, "id", s(id));
        }
        "connect" => {
            if pos.len() < 2 {
                return die("connect needs two object ids".into());
            }
            put(&mut b, "from", s(pos[0]));
            put(&mut b, "to", s(pos[1]));
            put(&mut b, "fields", flags.drain());
        }
        "template" => {
            let Some(name) = pos.first() else {
                return die(concat!(
                    "template needs a name (",
                    k2_client::cli_name!(),
                    " board templates)"
                )
                .into());
            };
            put(&mut b, "name", s(name));
            for k in ["x", "y"] {
                if let Some(v) = num(&mut flags, k)? {
                    put(&mut b, k, Value::Float(v));
                }
            }
        }
        "react" => {
            if pos.len() < 2 {
                return die("react needs an object id and an emoji".into());
            }
            put(&mut b, "id", s(pos[0]));
            put(&mut b, "emoji", s(pos[1]));
        }
        "vote" => {
            let Some(id) = pos.first() else {
                return die("vote needs an object id".into());
            };
            put(&mut b, "id", s(id));
        }
        "comment" => {
            if pos.len() < 2 {
                return die("comment needs an object id and text".into());
            }
            put(&mut b, "id", s(pos[0]));
            put(&mut b, "text", s(&pos[1..].join(" ")));
        }
        "meta" => {
            let f = flags.drain();
            if matches!(&f, Value::Object(p) if !p.is_empty()) {
                put(&mut b, "fields", f);
            }
        }
        "apply" => {
            let src = pos.first().copied().unwrap_or("-");
            let text = if src == "-" {
                String::from_utf8_lossy(&stdin()).into_owned()
            } else {
                pyjson::read_text_file(src)
                    .map_err(|e| Stop::Usage(format!("can not read {src}: {}", e.message)))?
            };
            let v = pyjson::loads(&text)
                .map_err(|e| Stop::Usage(format!("the ops are not JSON: {}", e.message)))?;
            let ops = match &v {
                Value::Object(pairs) => crate::families::get(pairs, "ops")
                    .cloned()
                    .unwrap_or(Value::Null),
                _ => v,
            };
            put(&mut b, "ops", ops);
        }
        "board-new" => {
            let Some(n) = take(&mut flags, "name")?.filter(|n| !n.is_empty()) else {
                return die("board-new needs --name".into());
            };
            put(&mut b, "name", s(&n));
            if let Some(p) = take(&mut flags, "project")?.filter(|p| !p.is_empty()) {
                put(&mut b, "project", s(&p));
            }
        }
        "board-rename" => {
            let Some(n) = take(&mut flags, "name")?.filter(|n| !n.is_empty()) else {
                return die("board-rename needs --name".into());
            };
            put(&mut b, "name", s(&n));
        }
        // templates, export
        _ => {}
    }
    if let Some(first) = flags.0.iter().map(|(k, _)| k).min() {
        return die(format!("{cmd} does not take --{first}"));
    }
    Ok(Ask {
        body: Value::Object(b),
        out: out.trim_end_matches('\n').to_string(),
    })
}

/// The body as `json.dumps` writes it.
pub fn payload(ask: &Ask) -> String {
    json::to_string(&ask.body, &Options::default()).expect("a body from words always prints")
}

/// `_board_fail <status> <resp>`: its lines and its exit.
pub fn fail(status: &str, resp: &str, as_json: bool) -> Shown {
    let parsed = pyjson::loads(resp).ok();
    let error = parsed.as_ref().and_then(|v| match v {
        Value::Object(pairs) => Some(crate::families::get(pairs, "error")),
        _ => None,
    });
    // The first Python, its print as `$(…)` holds it: `""` on any error.
    let code = match error {
        Some(Some(Value::Object(pairs))) => {
            crate::families::get(pairs, "code").map_or_else(String::new, pyfmt::str_of)
        }
        Some(e) => pyfmt::str_of(or(e, &Value::Str(Text::from("")))),
        None => String::new(),
    };
    let code = code.trim_end_matches('\n');
    let mut shown = Shown::err(String::new(), exit_of(code));
    if as_json && !resp.is_empty() {
        shown.stdout = format!("{resp}\n");
    } else if !resp.is_empty() {
        shown.stderr = match error {
            Some(Some(Value::Object(pairs))) => format!(
                "Error [{}]: {}\n",
                crate::families::get(pairs, "code").map_or_else(|| "error".into(), pyfmt::str_of),
                crate::families::get(pairs, "hint").map_or_else(String::new, pyfmt::str_of)
            ),
            Some(e) => format!("Error: {}\n", s_of(e)),
            None => {
                let head: String = resp.chars().take(200).collect();
                format!("Error: daemon returned: {head}\n")
            }
        };
    } else {
        shown.stderr = format!(
            concat!(
                "Error: no answer from the ",
                k2_client::product_name!(),
                " daemon (status {})\n"
            ),
            status
        );
    }
    shown
}

/// `_board_fail`'s `case "$code"`.
pub fn exit_of(code: &str) -> Exit {
    match code {
        "not_granted" | "core_off" | "person_only" | "owner_only" | "author_unknown" => {
            Exit::Refused
        }
        "no_board" | "no_object" | "no_template" | "no_project" | "blob_missing"
        | "newer_format" => Exit::NotFound,
        "locked" | "exists" | "voting_off" | "no_votes_left" | "board_trashed" => Exit::Conflict,
        "too_long" | "too_many" | "bad_field" | "unknown_field" | "unknown_type"
        | "value_too_large" | "bad_ops" | "bad_request" | "board_required" => Exit::Usage,
        _ => Exit::Error,
    }
}

/// The one-board warning (no `--board` named), when the answer names
/// the board it used.
pub fn warning(resp: &str) -> Option<String> {
    let Ok(Value::Object(pairs)) = pyjson::loads(resp) else {
        return None;
    };
    let b = crate::families::get(&pairs, "board")?;
    if !pyfmt::truthy(b) {
        return None;
    }
    let b = pyfmt::str_of(b);
    Some(format!(
        "warning: no --board, so this used your one board, {b}. Name it: --board {b}. A command with no board named is refused from the next release.\n"
    ))
}

/// The last line of the JSON reader's traceback, for an answer that is
/// not JSON.
fn not_json(e: &pyjson::Failed) -> Raise {
    Raise::at(1, e.last_line())
}

/// `--json`: the answer, `kind` set to `garden` when it is an object
/// without one.
pub fn as_json(resp: &str) -> Shown {
    // `printf '%s\n' "$resp" | python3 -c 'json.load(sys.stdin)…'`
    let mut v = match pyjson::loads(&format!("{resp}\n")) {
        Ok(v) => v,
        Err(e) => return raised(String::new(), String::new(), &not_json(&e)),
    };
    if let Value::Object(pairs) = &mut v {
        if crate::families::get(pairs, "kind").is_none() {
            pairs.push((Text::from("kind"), s("garden")));
        }
    }
    match dumps(&v, None) {
        Ok(text) => Shown::out(text + "\n"),
        Err(r) => raised(String::new(), String::new(), &r),
    }
}

/// `json.dumps(v, indent=…, ensure_ascii=False)`, or Python's error
/// writing a lone surrogate out.
fn dumps(v: &Value, indent: Option<usize>) -> Result<String, Raise> {
    json::to_string(
        v,
        &Options {
            indent,
            ensure_ascii: false,
            ..Options::default()
        },
    )
    .map_err(|e| Raise::at(1, format!("UnicodeEncodeError: {e}")))
}

/// The lines of the text reader that raise.
mod line {
    pub const LOAD: u32 = 3;
    pub const EXPORT: u32 = 6;
    pub const PATH: u32 = 7;
    pub const OPEN: u32 = 8;
    pub const DUMP: u32 = 9;
    pub const WROTE: u32 = 11;
    pub const BOARDS: u32 = 13;
    pub const BOARD: u32 = 15;
    pub const OBJECTS: u32 = 16;
    pub const COUNT: u32 = 18;
    pub const OBJECT: u32 = 19;
    pub const DELETED: u32 = 21;
    pub const LOG: u32 = 22;
    pub const LOG_ROW: u32 = 24;
    pub const TYPES: u32 = 25;
    pub const TEMPLATES: u32 = 27;
    pub const TEMPLATE: u32 = 28;
    pub const IDS_TEMPLATE: u32 = 29;
    pub const ADDED: u32 = 30;
    pub const IDS: u32 = 31;
    pub const APPLIED: u32 = 32;
    pub const META: u32 = 33;
    pub const DUMP_META: u32 = 34;
}

/// The text reader (`K2_BOARD_OUT=<out> python3 -c '…'` on the answer),
/// writing the export file in `cwd` when the answer holds one.
pub fn show(resp: &str, out: &str, cwd: &std::path::Path) -> Shown {
    let mut stdout = String::new();
    let mut stderr = String::new();
    match show_into(resp, out, cwd, &mut stdout, &mut stderr) {
        Ok(()) => Shown {
            stdout,
            stderr,
            exit: Exit::Ok,
        },
        Err(r) => raised(stdout, stderr, &r),
    }
}

fn show_into(
    resp: &str,
    out: &str,
    cwd: &std::path::Path,
    stdout: &mut String,
    stderr: &mut String,
) -> Result<(), Raise> {
    let v = pyjson::loads(resp).map_err(|e| Raise::at(line::LOAD, e.last_line()))?;
    let dump = |x: &Value, at: u32| {
        dumps(x, Some(2)).map_err(|r| Raise {
            frames: vec![(at, "<module>"), (4, "dump")],
            ..r
        })
    };
    let empty_list = Value::Array(Vec::new());
    let empty_dict = Value::Object(Vec::new());
    if contains(&v, "export", line::EXPORT)? {
        let path = if out.is_empty() {
            let board = match get_in(&v, "board", line::PATH)? {
                Some(b) => pyfmt::str_of(b),
                None => "board".to_string(),
            };
            format!("board-{board}.json")
        } else {
            out.to_string()
        };
        let e = index(&v, "export", line::DUMP)?;
        let text = dumps(e, Some(1)).map_err(|r| Raise::at(line::DUMP, r.exception))?;
        std::fs::write(cwd.join(&path), text + "\n").map_err(|err| {
            let f = pyjson::os_error(&err, &path);
            Raise::at(line::OPEN, format!("{}: {}", f.class, f.message))
        })?;
        let objects = py_len(
            or(get_in(e, "objects", line::WROTE)?, &empty_list),
            line::WROTE,
        )?;
        let blobs = py_len(
            or(get_in(e, "blobs", line::WROTE)?, &empty_dict),
            line::WROTE,
        )?;
        let people = py_len(
            or(get_in(e, "people", line::WROTE)?, &empty_dict),
            line::WROTE,
        )?;
        stdout.push_str(&format!(
            "Wrote {path}: {objects} objects, {blobs} images, {people} people\n"
        ));
    } else if contains(&v, "boards", line::BOARDS)? {
        let boards = index(&v, "boards", line::BOARDS + 1)?;
        if !pyfmt::truthy(boards) {
            stdout.push_str(
                "No boards you can reach. Your human grants one on its For agents panel.\n",
            );
        }
        for b in crate::families::py_iter(boards, line::BOARD - 1)? {
            let trashed = match get_in(&b, "trashed", line::BOARD)? {
                Some(t) if pyfmt::truthy(t) => "  (in the Trash)",
                _ => "",
            };
            let id = pyfmt::str_of(index(&b, "id", line::BOARD)?);
            let name = pyfmt::str_of(index(&b, "name", line::BOARD)?);
            stdout.push_str(&format!("{id}  {name}{trashed}\n"));
        }
    } else if contains(&v, "objects", line::OBJECTS)? {
        stdout
            .push_str(&(dump(index(&v, "objects", line::OBJECTS + 1)?, line::OBJECTS + 1)? + "\n"));
        if contains(&v, "count", line::COUNT)? {
            let n = py_d(index(&v, "count", line::COUNT)?, line::COUNT)?;
            let board = s_of(get_in(&v, "board", line::COUNT)?);
            stderr.push_str(&format!("{n} objects on board {board}\n"));
        }
    } else if contains(&v, "object", line::OBJECT)? {
        stdout.push_str(&(dump(index(&v, "object", line::OBJECT + 1)?, line::OBJECT + 1)? + "\n"));
    } else if contains(&v, "deleted", line::DELETED - 1)? {
        let list = join(index(&v, "deleted", line::DELETED)?, ", ", line::DELETED)?;
        stdout.push_str(&format!("Deleted {list}\n"));
    } else if contains(&v, "log", line::LOG)? {
        for r in crate::families::py_iter(index(&v, "log", line::LOG + 1)?, line::LOG + 1)? {
            let rev = pyfmt::str_of(index(&r, "rev", line::LOG_ROW)?);
            let at = pyfmt::str_of(index(&r, "at", line::LOG_ROW)?);
            let by = pyfmt::str_of(index(&r, "by", line::LOG_ROW)?);
            let changed = join(index(&r, "changed", line::LOG_ROW)?, ", ", line::LOG_ROW)?;
            let deleted = match get_in(&r, "deleted", line::LOG_ROW)? {
                Some(d) if pyfmt::truthy(d) => format!(
                    "  ({} deleted)",
                    py_d(index(&r, "deleted", line::LOG_ROW)?, line::LOG_ROW)?
                ),
                _ => String::new(),
            };
            stdout.push_str(&format!("rev {rev}  {at}  {by}  {changed}{deleted}\n"));
        }
    } else if contains(&v, "types", line::TYPES)? {
        stdout.push_str(&(dump(index(&v, "types", line::TYPES + 1)?, line::TYPES + 1)? + "\n"));
    } else if contains(&v, "templates", line::TEMPLATES)? {
        for t in crate::families::py_iter(index(&v, "templates", line::TEMPLATE)?, line::TEMPLATE)?
        {
            let name = index(&t, "name", line::TEMPLATE)?;
            let label = pyfmt::str_of(index(&t, "label", line::TEMPLATE)?);
            stdout.push_str(&format!("{} {label}\n", crate::families::py_left(name, 12)));
        }
    } else if contains(&v, "ids", line::IDS_TEMPLATE)?
        && contains(&v, "template", line::IDS_TEMPLATE)?
    {
        let template = pyfmt::str_of(index(&v, "template", line::ADDED)?);
        let n = py_len(index(&v, "ids", line::ADDED)?, line::ADDED)?;
        stdout.push_str(&format!("Added {template}: {n} objects\n"));
    } else if contains(&v, "ids", line::IDS)? {
        stdout.push_str(&(dump(index(&v, "ids", line::APPLIED)?, line::APPLIED)? + "\n"));
        stderr.push_str(&format!(
            "{} ops applied\n",
            s_of(get_in(&v, "count", line::APPLIED)?)
        ));
    } else if contains(&v, "meta", line::META)? {
        stdout.push_str(&(dump(index(&v, "meta", line::DUMP_META)?, line::DUMP_META)? + "\n"));
    } else {
        stdout.push_str(&(dump(&v, line::DUMP_META + 2)? + "\n"));
    }
    Ok(())
}

/// What the garden form prints for what `_compute_send` gave.
pub fn answer(
    sent: &Sent,
    named: bool,
    as_json_flag: bool,
    out: &str,
    cwd: &std::path::Path,
) -> Shown {
    let resp = String::from_utf8_lossy(&sent.resp).into_owned();
    if sent.status != "200" {
        return fail(&sent.status, &resp, as_json_flag);
    }
    let lead = if named { None } else { warning(&resp) };
    let mut shown = if as_json_flag {
        as_json(&resp)
    } else {
        show(&resp, out, cwd)
    };
    if let Some(w) = lead {
        shown.stderr = w + &shown.stderr;
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(words: &[&str]) -> String {
        payload(&parse(words, Vec::new).unwrap())
    }

    fn usage(words: &[&str]) -> String {
        match parse(words, Vec::new).unwrap_err() {
            Stop::Usage(m) => m,
            Stop::Raised(r) => panic!("{words:?} raised {r:?}"),
        }
    }

    /// The blind agent's words (`tests/cli/board_cli.sh`) and their bodies.
    #[test]
    fn the_words_make_the_shell_s_bodies() {
        assert_eq!(body(&["boards"]), r#"{"cmd": "boards"}"#);
        assert_eq!(
            body(&["read", "h-7f3a2b1c9d", "--board", "Launch"]),
            r#"{"cmd": "read", "board": "Launch", "id": "h-7f3a2b1c9d"}"#
        );
        assert_eq!(
            body(&[
                "add",
                "sticky",
                "--text",
                "Ship the beta",
                "--x",
                "400",
                "--y",
                "300",
                "--color",
                "yellow",
                "--board",
                "Launch"
            ]),
            r#"{"cmd": "add", "board": "Launch", "type": "sticky", "fields": {"text": "Ship the beta", "x": "400", "y": "300", "color": "yellow"}}"#
        );
        assert_eq!(
            body(&["connect", "a", "b", "--label", "next", "--json"]),
            r#"{"cmd": "connect", "from": "a", "to": "b", "fields": {"label": "next"}}"#
        );
        assert_eq!(
            body(&["add"]),
            r#"{"cmd": "add", "type": "sticky", "fields": {}}"#
        );
        assert_eq!(
            body(&["template", "kanban", "--x", "10", "--y=-2.5e1"]),
            r#"{"cmd": "template", "name": "kanban", "x": 10.0, "y": -25.0}"#
        );
        assert_eq!(
            body(&["log", "--limit", "7.9"]),
            r#"{"cmd": "log", "limit": 7}"#
        );
        assert_eq!(
            body(&["ls", "--type", "", "--full"]),
            r#"{"cmd": "list", "full": true}"#
        );
        assert_eq!(
            body(&["add", "note", "--a", "1", "--b", "--a", "2", "--é"]),
            r#"{"cmd": "add", "type": "note", "fields": {"a": "2", "b": true, "\u00e9": true}}"#
        );
        assert_eq!(
            body(&["new", "--name", "Plan"]),
            r#"{"cmd": "board-new", "name": "Plan"}"#
        );
        assert_eq!(
            body(&["comment", "x", "a", "b"]),
            r#"{"cmd": "comment", "id": "x", "text": "a b"}"#
        );
    }

    #[test]
    fn the_shell_s_usage_lines() {
        assert_eq!(usage(&["zap"]), "unknown command zap");
        assert_eq!(
            usage(&["update", "x"]),
            "update needs at least one --<field> <value>"
        );
        assert_eq!(usage(&["update"]), "update needs an object id");
        assert_eq!(
            usage(&["boards", "--zz", "--aa"]),
            "boards does not take --aa"
        );
        assert_eq!(
            usage(&["list", "--board"]),
            "--board takes a board id or name"
        );
        assert_eq!(
            usage(&["template", "k", "--x", "ten"]),
            "--x takes a number"
        );
        assert_eq!(usage(&["board-new", "--name"]), "--name takes a value");
        assert_eq!(
            usage(&["apply", "-"]),
            "the ops are not JSON: Expecting value: line 1 column 1 (char 0)"
        );
        assert_eq!(
            usage(&["apply", "/no/such/file"]),
            "can not read /no/such/file: [Errno 2] No such file or directory: '/no/such/file'"
        );
    }

    #[test]
    fn a_limit_python_can_t_make_an_int_raises() {
        let Err(Stop::Raised(r)) = parse(&["log", "--limit", "inf"], Vec::new) else {
            panic!("no raise");
        };
        assert_eq!(
            r.exception,
            "OverflowError: cannot convert float infinity to integer"
        );
    }

    #[test]
    fn apply_reads_its_ops() {
        let ask = parse(&["apply"], || {
            br#"{"ops": [{"op": "add", "x": 1.50}]}"#.to_vec()
        })
        .unwrap();
        assert_eq!(
            payload(&ask),
            r#"{"cmd": "apply", "ops": [{"op": "add", "x": 1.5}]}"#
        );
        let ask = parse(&["apply"], || b"[1]".to_vec()).unwrap();
        assert_eq!(payload(&ask), r#"{"cmd": "apply", "ops": [1]}"#);
    }

    #[test]
    fn the_refusals_and_their_exits() {
        let r = fail(
            "409",
            r#"{"ok":false,"error":{"code":"locked","hint":"h is locked"}}"#,
            false,
        );
        assert_eq!(
            (r.stderr.as_str(), r.exit),
            ("Error [locked]: h is locked\n", Exit::Conflict)
        );
        let r = fail("403", r#"{"error":"nope"}"#, false);
        assert_eq!((r.stderr.as_str(), r.exit), ("Error: nope\n", Exit::Error));
        let r = fail("000", "\n000", false);
        assert_eq!(r.stderr, "Error: daemon returned: \n000\n");
        let r = fail("500", "", false);
        assert_eq!(
            r.stderr,
            concat!(
                "Error: no answer from the ",
                k2_client::product_name!(),
                " daemon (status 500)\n"
            )
        );
        let r = fail("403", r#"{"error":{"code":"core_off"}}"#, true);
        assert_eq!(
            (r.stdout.as_str(), r.stderr.as_str(), r.exit),
            ("{\"error\":{\"code\":\"core_off\"}}\n", "", Exit::Refused)
        );
    }

    #[test]
    fn what_the_text_reader_prints() {
        let cwd = std::env::temp_dir();
        let r = show(
            r#"{"ok":true,"boards":[{"id":"b1","name":"Launch","trashed":true}]}"#,
            "",
            &cwd,
        );
        assert_eq!(r.stdout, "b1  Launch  (in the Trash)\n");
        let r = show(
            r#"{"ok":true,"board":"b","rev":9,"deleted":["a","c"]}"#,
            "",
            &cwd,
        );
        assert_eq!(r.stdout, "Deleted a, c\n");
        let r = show(
            r#"{"objects":[{"id":"é"}],"count":1,"board":"b"}"#,
            "",
            &cwd,
        );
        assert_eq!(r.stdout, "[\n  {\n    \"id\": \"é\"\n  }\n]\n");
        assert_eq!(r.stderr, "1 objects on board b\n");
        let r = show(
            r#"{"templates":[{"name":"kanban","label":"Kanban"}]}"#,
            "",
            &cwd,
        );
        assert_eq!(r.stdout, "kanban       Kanban\n");
        let r = show("[1]", "", &cwd);
        assert_eq!(r.stdout, "[\n  1\n]\n");
        let r = show("5", "", &cwd);
        assert!(
            r.stderr
                .ends_with("TypeError: argument of type 'int' is not iterable\n"),
            "{r:?}"
        );
        assert_eq!(r.exit, Exit::Error);
    }

    #[test]
    fn the_warning_names_the_board() {
        assert_eq!(
            warning(r#"{"board":"b1"}"#).unwrap(),
            "warning: no --board, so this used your one board, b1. Name it: --board b1. A command with no board named is refused from the next release.\n"
        );
        assert_eq!(warning(r#"{"board":""}"#), None);
        assert_eq!(warning("x"), None);
    }
}
