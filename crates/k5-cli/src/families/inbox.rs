//! `k5 inbox` and its verbs, as the shell CLI's `cmd_inbox` answers them
//! (`cli/src/inbox.sh`): the reads `list` (also no verb, and a first word
//! that is no verb), `read <id>`, `search "query"` and `folders`, and the
//! writes `compose`, `respond`, `move`, `archive` and `delete`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `inbox` is an `id` tool in the shell's policy, so with `HOOK_SOCK`
//!    set: a socket that is gone with a passport the probe finds refused
//!    is the stale-passport lines, and no passport is the
//!    require-passport lines; both exit 3 ([`crate::prelude::id_tool`]).
//! 3. The words ([`parse`]). `read` with no id is `Usage: k2 inbox read
//!    <id>`, exit 2; `search` with no query is `Usage: k2 inbox search
//!    "query"`, exit 1. A value flag as the last word (`list --folder`,
//!    and `compose`'s `--title`, `--body`, `--priority`, `--source`,
//!    `--from` and `--summary`) is [`out::need_value`]'s line and exit 2,
//!    nothing sent: the shell's `_need_value` line, the same.
//! 4. One `cli_send cli_request <route> [<key>=<escaped>]` on the left of
//!    a pipe into the shell's Python ([`super::request_piped`]): the cell
//!    socket first when there is a passport with a live socket (every
//!    `/cli/inbox/` route is one it serves), then TCP when that gave no
//!    usable answer.
//!    - `list` and `search`: `format_inbox` ([`format_inbox`]), whose
//!      errors are thrown away (`2>/dev/null || cat`), so its part always
//!      ends 0: the command ends with the call's code.
//!    - `read`: the item's `content`, or the error's line with exit 4 (not
//!      found) or 1 ([`read_item`]).
//!    - `folders`: one line per folder, or the two "No folders yet" lines
//!      ([`folders`]); a folder that is not a string ends it with 1.
//!
//!    A write is one `cli_send cli_post_form <route> <form>` then `echo`
//!    ([`write`]): sent once, as the shell's no-resend rule says (a POST
//!    goes on to TCP only when the cell socket refused the connect or
//!    doesn't serve the route), the answer as it came, and a newline. A
//!    missing word is the verb's usage line, exit 1.
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]), with the
//!    pipeline's code (`set -o pipefail`, [`super::pipefail`]).
//!
//! A request with no answer is the transport's line, and the command then
//! dies of SIGPIPE with nothing else printed, as the shell's
//! `_k2_end_command` ends a pipe ([`super::end_in_pipe`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`):
//!
//! - a command line with `-h` or `--help` (the front sends those on,
//!   [`crate::front`]) and a word that is not UTF-8;
//! - `HOOK_SOCK` set with a `CODEX_` variable exported (the shell looks
//!   for a shared Codex server first, [`crate::prelude::codex_env`]);
//! - a port that is set but is no port curl takes (the shell's curl fails
//!   on TCP where its cell socket may still answer);
//! - a `python3` the binary can't tell ([`crate::families`], Without Python).
//!
//! Where the shell's Python itself fails with a traceback (`read`: a hint
//! or code that is not a string), the text depends on the Python version;
//! this prints the Python 3.12 one, without source lines. An answer that
//! is not UTF-8 is read here with U+FFFD for each bad byte (Python's
//! depends on its locale: a traceback, or the bytes read through); a
//! string with half of a surrogate pair is printed as `\udXXX` where
//! Python's `print` fails. The daemon sends neither.

use super::{curl_port, get, pipefail, request_piped, traceback, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Call, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;
use std::path::Path;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["inbox"];
/// The binary answers it; [`run`] hands back a form only the shell answers.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes.
pub const LIST: &str = "/cli/inbox/list";
pub const READ: &str = "/cli/inbox/read";
pub const SEARCH: &str = "/cli/inbox/search";
pub const FOLDERS: &str = "/cli/inbox/folders";
pub const COMPOSE: &str = "/cli/inbox/compose";
pub const RESPOND: &str = "/cli/inbox/respond";
pub const MOVE: &str = "/cli/inbox/move";
pub const ARCHIVE: &str = "/cli/inbox/archive";
pub const DELETE: &str = "/cli/inbox/delete";
/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "inbox";
/// `read` with no id: exit 2.
pub const READ_USAGE: &str = concat!("Usage: ", cli_name!(), " inbox read <id>");
/// `search` with no query: exit 1.
pub const SEARCH_USAGE: &str = concat!("Usage: ", cli_name!(), " inbox search \"query\"");
/// The writes' usage lines, each exit 1.
pub const COMPOSE_USAGE: &str = concat!(
    "Usage: ",
    k2_client::cli_name!(),
    " inbox compose --title \"...\" [--body \"...\"] [--priority H|N|L]"
);
pub const RESPOND_USAGE: &str = concat!(
    "Usage: ",
    k2_client::cli_name!(),
    " inbox respond <id> \"text\""
);
pub const MOVE_USAGE: &str = concat!(
    "Usage: ",
    k2_client::cli_name!(),
    " inbox move <id> <folder>"
);
pub const ARCHIVE_USAGE: &str = concat!("Usage: ", k2_client::cli_name!(), " inbox archive <id>");
pub const DELETE_USAGE: &str = concat!("Usage: ", k2_client::cli_name!(), " inbox delete <id>");
/// `format_inbox` on an empty inbox.
pub const NO_ITEMS: &str = "No items.";
/// `folders` with none.
pub const NO_FOLDERS: [&str; 2] = [
    "No folders yet. Create one by moving an item:",
    concat!("  ", cli_name!(), " inbox move <id> projects"),
];
/// `read`'s Python on an answer that is empty or only spaces.
pub const READ_NO_ANSWER: &str = concat!(
    "Error: no answer from the ",
    k2_client::product_name!(),
    " daemon (neither this session's cell socket nor the loopback port answered). Is ",
    k2_client::product_name!(),
    " running? Try again: ",
    cli_name!(),
    " inbox read <id>"
);
/// `read`'s Python on an answer that is not JSON, before its `%r`.
pub const READ_UNUSABLE: &str = concat!(
    "Error: the ",
    k2_client::product_name!(),
    " daemon gave no usable answer: "
);
/// The line of `read`'s Python that calls `hint.lower()`, for its
/// traceback (`tests` pins it to the script).
pub const PY_READ_LOWER: u32 = 16;

/// What the words ask for. An empty id or query is the usage line, after
/// the gates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// `folder=` only when the folder is not empty.
    List {
        folder: String,
    },
    Read {
        id: String,
    },
    Search {
        q: String,
    },
    Folders,
    /// `compose`: the form (`title=…` and the other fields set), or `None`
    /// with no title.
    Compose {
        form: Option<String>,
    },
    /// `respond`, `move`, `archive`, `delete`: the route, the form's pairs
    /// (`None`: a word is missing) and the usage line.
    Write {
        route: &'static str,
        pairs: Option<Vec<String>>,
        usage: &'static str,
    },
    /// A value flag as the last word (`list --folder`, a `compose` value
    /// flag), as typed.
    NeedsValue(String),
}

/// The words after `inbox`, as `cmd_inbox` and its verbs read them.
/// `None`: the shell's (`-h` or `--help` as the verb).
pub fn parse(words: &[&str]) -> Option<Ask> {
    let first = |rest: &[&str]| rest.first().map_or_else(String::new, |w| (*w).to_string());
    match words.first() {
        None => Some(Ask::List {
            folder: String::new(),
        }),
        Some(&"list") => list(&words[1..]),
        Some(&"read") => Some(Ask::Read {
            id: first(&words[1..]),
        }),
        Some(&"search") => Some(Ask::Search {
            q: first(&words[1..]),
        }),
        Some(&"folders") => Some(Ask::Folders),
        Some(&"compose") => compose(&words[1..]),
        Some(&"respond") => {
            // The first bare `--` is dropped: the words after it are the id
            // and the text, as typed.
            let mut rest: Vec<&str> = words[1..].to_vec();
            if let Some(at) = rest.iter().position(|w| *w == "--") {
                rest.remove(at);
            }
            Some(two(RESPOND, "id", "text", &rest, RESPOND_USAGE))
        }
        Some(&"move") => Some(two(MOVE, "id", "folder", &words[1..], MOVE_USAGE)),
        Some(&"archive") => Some(one(ARCHIVE, &words[1..], ARCHIVE_USAGE)),
        Some(&"delete") => Some(one(DELETE, &words[1..], DELETE_USAGE)),
        Some(&("--help" | "-h")) => None,
        // `-*|*`: the whole line is `inbox list`'s.
        Some(_) => list(words),
    }
}

/// `cmd_inbox_compose`'s loop: each value flag takes the next word (none:
/// [`Ask::NeedsValue`]), any other word is skipped.
fn compose(words: &[&str]) -> Option<Ask> {
    const FIELDS: [&str; 6] = ["title", "body", "priority", "source", "from", "summary"];
    let mut vals: [&str; 6] = [""; 6];
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match w
            .strip_prefix("--")
            .and_then(|f| FIELDS.iter().position(|x| *x == f))
        {
            Some(k) => {
                let Some(v) = words.get(i + 1) else {
                    return Some(Ask::NeedsValue((*w).to_string()));
                };
                vals[k] = v;
                i += 2;
            }
            None => i += 1,
        }
    }
    if vals[0].is_empty() {
        return Some(Ask::Compose { form: None });
    }
    let mut form = format!("title={}", quote(vals[0]));
    for (k, v) in FIELDS.iter().zip(vals).skip(1) {
        if !v.is_empty() {
            form.push_str(&format!("&{k}={}", quote(v)));
        }
    }
    Some(Ask::Compose { form: Some(form) })
}

/// `id="${1:-}" <b>="${2:-}"`: both must be there.
fn two(route: &'static str, a: &str, b: &str, words: &[&str], usage: &'static str) -> Ask {
    let x = words.first().copied().unwrap_or("");
    let y = words.get(1).copied().unwrap_or("");
    let pairs = (!x.is_empty() && !y.is_empty())
        .then(|| vec![format!("{a}={}", quote(x)), format!("{b}={}", quote(y))]);
    Ask::Write {
        route,
        pairs,
        usage,
    }
}

/// `id="${1:-}"`: it must be there.
fn one(route: &'static str, words: &[&str], usage: &'static str) -> Ask {
    let x = words.first().copied().unwrap_or("");
    Ask::Write {
        route,
        pairs: (!x.is_empty()).then(|| vec![format!("id={}", quote(x))]),
        usage,
    }
}

/// `cmd_inbox_list`'s loop: `--folder <f>` sets it, any other word
/// starting with `-` is skipped, and a word is the folder while none is
/// set yet.
fn list(words: &[&str]) -> Option<Ask> {
    let mut folder = String::new();
    let mut i = 0;
    while let Some(w) = words.get(i) {
        match *w {
            "--folder" => {
                let Some(v) = words.get(i + 1) else {
                    return Some(Ask::NeedsValue("--folder".into()));
                };
                folder = (*v).to_string();
                i += 2;
            }
            w if w.starts_with('-') => i += 1,
            w => {
                if folder.is_empty() {
                    folder = w.to_string();
                }
                i += 1;
            }
        }
    }
    Some(Ask::List { folder })
}

/// `k5 inbox ...`: `args` is the whole command line after `argv[0]`.
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
    Some(run_with(args, &ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    ask: &Ask,
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
    match ask {
        Ask::NeedsValue(flag) => return out::stop_need_value(flag),
        Ask::Read { id } if id.is_empty() => {
            out::stderr_line(READ_USAGE);
            return Exit::Usage;
        }
        Ask::Search { q } if q.is_empty() => {
            out::stderr_line(SEARCH_USAGE);
            return Exit::Error;
        }
        Ask::Compose { form: None } => {
            out::stderr_line(COMPOSE_USAGE);
            return Exit::Error;
        }
        Ask::Write {
            pairs: None, usage, ..
        } => {
            out::stderr_line(usage);
            return Exit::Error;
        }
        _ => {}
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let (endpoint, arg) = match ask {
        Ask::Compose { form: Some(form) } => {
            return write(&mut client, closing, COMPOSE, &[form.as_str()]);
        }
        Ask::Write {
            route,
            pairs: Some(pairs),
            ..
        } => {
            let pairs: Vec<&str> = pairs.iter().map(String::as_str).collect();
            return write(&mut client, closing, route, &pairs);
        }
        Ask::Compose { form: None } | Ask::Write { pairs: None, .. } | Ask::NeedsValue(_) => {
            unreachable!("a usage line above")
        }
        Ask::List { folder } => (
            LIST,
            (!folder.is_empty()).then(|| format!("folder={}", quote(folder))),
        ),
        Ask::Read { id } => (READ, Some(format!("id={}", quote(id)))),
        Ask::Search { q } => (SEARCH, Some(format!("q={}", quote(q)))),
        Ask::Folders => (FOLDERS, None),
    };
    let call_args: Vec<&str> = arg.iter().map(String::as_str).collect();
    let piped = request_piped(&mut client, &mut closing, endpoint, &call_args);
    let shown = match ask {
        Ask::List { .. } | Ask::Search { .. } => Shown::out(format_inbox(&piped.body)),
        Ask::Read { .. } => read_item(&piped.body),
        Ask::Folders => folders(&piped.body),
        Ask::Compose { .. } | Ask::Write { .. } | Ask::NeedsValue(_) => {
            unreachable!("a write returned above")
        }
    };
    let exit = pipefail(piped.exit, shown.print());
    closing.flush(&mut client, exit);
    exit
}

/// `cli_send cli_post_form <route> <pairs>; echo`: the answer as it came
/// (its trailing newlines dropped when it came over the cell socket, held
/// in `$(…)`), then a newline. No answer, or a send that may have run and
/// is not sent again, is the transport's line and exit 1; any other
/// failure is curl's code, with nothing printed. The EXIT trap's lines
/// close it either way.
fn write(client: &mut Client, mut closing: Closing, route: &str, pairs: &[&str]) -> Exit {
    let exit = match client.send(route, Call::PostForm(pairs)) {
        Ok(r) => {
            closing.answer(&r, client.passport().is_some());
            let mut body = super::piped_body(&r);
            body.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &body) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Err(
            e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. }),
        ) => {
            out::stderr_line(&e.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(client, exit);
    exit
}

/// A value Python's `print` writes out: not a string with half of a
/// surrogate pair in it (`repr` escapes those, so only a string's own
/// `str()` holds one).
fn printable(v: &Value) -> bool {
    !matches!(v, Value::Str(t) if t.as_str().is_none())
}

/// `f'{v:<width}'`: the value's text padded with spaces to `width` code
/// points. `None` where Python raises: `None`, a list or a dict ("unsupported
/// format string"). A bool is its number, as `int.__format__` writes it.
fn left(v: &Value, width: usize) -> Option<String> {
    let text = match v {
        Value::Str(_) => py::str_of(v),
        Value::Int(d) => d.clone(),
        Value::Bool(b) => (if *b { "1" } else { "0" }).to_string(),
        Value::Float(_) => py::repr(v),
        Value::Null | Value::Array(_) | Value::Object(_) => return None,
    };
    let n = text.chars().count();
    Some(if n < width {
        format!("{text}{}", " ".repeat(width - n))
    } else {
        text
    })
}

/// One item's lines as `format_inbox` prints them, each `None` where its
/// Python raises (nothing more is printed after that).
fn item_lines(item: &Value) -> [Option<String>; 2] {
    let Value::Object(pairs) = item else {
        return [None, None];
    };
    let or = |key: &str, default: &str| {
        get(pairs, key)
            .cloned()
            .unwrap_or_else(|| Value::Str(Text::from(default)))
    };
    let folder = Some(or("folder", ""))
        .filter(py::truthy)
        .unwrap_or_else(|| Value::Str(Text::from("(top)")));
    let priority = or("priority", "normal");
    let title = get(pairs, "title")
        .filter(|v| py::truthy(v))
        .cloned()
        .unwrap_or_else(|| or("filename", ""));
    let iid = or("id", "?");
    // A dict lookup: a list or a dict is unhashable.
    if matches!(priority, Value::Array(_) | Value::Object(_)) {
        return [None, None];
    }
    let mark = match &priority {
        Value::Str(t) => match t.as_str() {
            Some("high") => "!",
            Some("critical") => "!!",
            Some("low") => ".",
            _ => " ",
        },
        _ => " ",
    };
    let line = match (left(&iid, 24), left(&folder, 10)) {
        (Some(i), Some(f)) if printable(&iid) && printable(&folder) && printable(&title) => {
            Some(format!("  {i} [{f}] {mark} {}\n", py::str_of(&title)))
        }
        _ => None,
    };
    let summary = match get(pairs, "summary").filter(|v| py::truthy(v)) {
        Some(s) if printable(s) => Some(format!("  {:<24}   summary: {}\n", "", py::str_of(s))),
        Some(_) => None,
        // Nothing to print is no failure.
        None => Some(String::new()),
    };
    [line, summary]
}

/// `format_inbox`'s stdout for the answer it reads. Its errors go to
/// `/dev/null` and `|| cat` then prints what is left of its input, which
/// is nothing (Python read it all), so its exit is always 0.
pub fn format_inbox(body: &[u8]) -> String {
    let Ok(text) = std::str::from_utf8(body) else {
        return String::new();
    };
    let Ok(data) = json::parse(if text.is_empty() { "[]" } else { text }) else {
        return String::new();
    };
    if !py::truthy(&data) {
        return format!("{NO_ITEMS}\n");
    }
    // A string, a dict (its keys) or a number: the first item, or the
    // loop itself, raises.
    let Value::Array(items) = &data else {
        return String::new();
    };
    let mut out = String::new();
    for item in items {
        for line in item_lines(item) {
            match line {
                Some(l) => out.push_str(&l),
                None => return out,
            }
        }
    }
    out
}

/// `folders`' Python: one `'  ' + f` per folder, or the two lines for
/// none. Not JSON (or not UTF-8) counts as none. Its errors go to
/// `/dev/null`: a folder that is not a string ends it with exit 1, after
/// the folders before it.
pub fn folders(body: &[u8]) -> Shown {
    let data = std::str::from_utf8(body)
        .ok()
        .and_then(|t| json::parse(if t.is_empty() { "[]" } else { t }).ok())
        .unwrap_or(Value::Array(Vec::new()));
    if !py::truthy(&data) {
        return Shown::out(format!("{}\n{}\n", NO_FOLDERS[0], NO_FOLDERS[1]));
    }
    let names: Vec<Value> = match data {
        Value::Array(items) => items,
        Value::Str(t) => t
            .code_points()
            .into_iter()
            .map(|cp| Value::Str(one_code_point(cp)))
            .collect(),
        Value::Object(pairs) => pairs.into_iter().map(|(k, _)| Value::Str(k)).collect(),
        // A number or true: not iterable.
        _ => return Shown::err("", Exit::Error),
    };
    let mut stdout = String::new();
    for name in &names {
        match name {
            Value::Str(t) if t.as_str().is_some() => {
                stdout.push_str("  ");
                stdout.push_str(&py::str_of(name));
                stdout.push('\n');
            }
            _ => {
                return Shown {
                    stdout,
                    stderr: String::new(),
                    exit: Exit::Error,
                }
            }
        }
    }
    Shown::out(stdout)
}

/// A one-code-point string, half of a surrogate pair too.
fn one_code_point(cp: u32) -> Text {
    match char::from_u32(cp) {
        Some(c) => Text::from(c.to_string()),
        None => match json::parse(&format!("\"\\u{cp:04x}\"")) {
            Ok(Value::Str(t)) => t,
            _ => Text::from("\u{fffd}"),
        },
    }
}

/// `read`'s Python on the answer it reads.
pub fn read_item(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    if raw.trim_matches(super::py_space).is_empty() {
        return Shown::err(format!("{READ_NO_ANSWER}\n"), Exit::Error);
    }
    let Ok(d) = json::parse(&raw) else {
        let head: String = raw.chars().take(200).collect();
        return Shown::err(
            format!(
                "{READ_UNUSABLE}{}\n",
                py::repr(&Value::Str(Text::from(head)))
            ),
            Exit::Error,
        );
    };
    let unexpected = Value::Str(Text::from("unexpected answer"));
    let err = match &d {
        Value::Object(pairs) => get(pairs, "error").filter(|e| **e != Value::Null),
        _ => Some(&unexpected),
    };
    if let Some(err) = err {
        let hint = match err {
            Value::Object(e) => [get(e, "hint"), get(e, "code")]
                .into_iter()
                .flatten()
                .find(|v| py::truthy(v))
                .cloned()
                .unwrap_or_else(|| Value::Str(Text::from(py::str_of(err)))),
            other => Value::Str(Text::from(py::str_of(other))),
        };
        let line = format!("Error: {}\n", py::str_of(&hint));
        let Value::Str(h) = &hint else {
            let exception = format!(
                "AttributeError: '{}' object has no attribute 'lower'",
                py::type_name(&hint)
            );
            return Shown::err(
                line + &traceback(&[(PY_READ_LOWER, "<module>")], &exception),
                Exit::Error,
            );
        };
        let lowered: String = py::str_of(&Value::Str(h.clone())).to_lowercase();
        let not_found = lowered.contains("not found")
            || matches!(err, Value::Object(e) if get(e, "code") == Some(&Value::Str(Text::from("not_found"))));
        return Shown::err(
            line,
            if not_found {
                Exit::NotFound
            } else {
                Exit::Error
            },
        );
    }
    let Value::Object(pairs) = &d else {
        // Not a dict is the "unexpected answer" above.
        return Shown::err("", Exit::Error);
    };
    let content = get(pairs, "content")
        .cloned()
        .unwrap_or_else(|| Value::Str(Text::from("")));
    Shown::out(format!("{}\n", py::str_of(&content)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/inbox.sh");
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Each line, request and verb, as `cmd_inbox` has them.
    #[test]
    fn the_shell_s_lines() {
        let text = source();
        for shell in [
            r#"            --folder) _need_value "$@"; folder="$2"; shift 2 ;;"#,
            r#"            -*) shift ;;"#,
            r#"            *) [[ -z "$folder" ]] && folder="$1"; shift ;;"#,
            r#"    [[ -n "$folder" ]] && params="folder=$(urlencode "$folder")""#,
            r#"        cli_send cli_request "/cli/inbox/list" "$params" | format_inbox"#,
            r#"        cli_send cli_request "/cli/inbox/list" | format_inbox"#,
            r#"        echo "Usage: k2 inbox read <id>" >&2"#,
            r#"        exit 2"#,
            r#"    cli_send cli_request "/cli/inbox/read" "id=$(urlencode "$id")" | python3 -c ""#,
            r#"        echo "Usage: k2 inbox search \"query\"" >&2"#,
            r#"    cli_send cli_request "/cli/inbox/search" "q=$(urlencode "$q")" | format_inbox"#,
            r#"    cli_send cli_request "/cli/inbox/folders" | python3 -c ""#,
            r#"" 2>/dev/null || cat"#,
            r#"    print('No items.')"#,
            r#"    print('No folders yet. Create one by moving an item:')"#,
            r#"    print('  k2 inbox move <id> projects')"#,
            r#"        print('  ' + f)"#,
            r#"    folder = item.get('folder', '') or '(top)'"#,
            r#"    priority = item.get('priority', 'normal')"#,
            r#"    title = item.get('title') or item.get('filename', '')"#,
            r#"    iid = item.get('id', '?')"#,
            r#"    prio_mark = {'high': '!', 'critical': '!!', 'normal': ' ', 'low': '.'}"#,
            r#"    print(f'  {iid:<24} [{folder:<10}] {mark} {title}')"#,
            r#"        print(f'  {\"\":<24}   summary: {item[\"summary\"]}')"#,
            r#"        list)     shift; cmd_inbox_list "$@" ;;"#,
            r#"        read)     shift; cmd_inbox_read "$@" ;;"#,
            r#"        compose)  shift; cmd_inbox_compose "$@" ;;"#,
            r#"        respond)  shift; cmd_inbox_respond "$@" ;;"#,
            r#"        move)     shift; cmd_inbox_move "$@" ;;"#,
            r#"        archive)  shift; cmd_inbox_archive "$@" ;;"#,
            r#"        delete)   shift; cmd_inbox_delete "$@" ;;"#,
            r#"        search)   shift; cmd_inbox_search "$@" ;;"#,
            r#"        folders)  cmd_inbox_folders ;;"#,
            r#"        --help|-h) cmd_help_inbox ;;"#,
            r#"            cmd_inbox_list "$@""#,
        ] {
            assert!(
                text.lines().any(|l| l == shell),
                "cmd_inbox no longer has {shell:?}"
            );
        }
        assert_eq!(
            text.matches("cli_send cli_request").count(),
            5,
            "a read form's request moved, or a new one came"
        );
        let policy = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../cli/src/cli-tools-policy.sh"),
        )
        .unwrap();
        assert!(
            policy.contains(r#"        inbox) echo "inbox" ;;"#),
            "inbox's tool id moved"
        );
        assert!(
            policy.contains("        msg|inbox|respond|awareness|"),
            "inbox is no longer an id tool"
        );
        let cell = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/cell-socket.sh"),
        )
        .unwrap();
        assert!(
            cell.contains("        /cli/inbox/*|/cli/review-checklist/*) return 0 ;;"),
            "the cell socket no longer serves /cli/inbox/*"
        );
    }

    /// `read`'s Python, its lines as `python3 -c` numbers them.
    #[test]
    fn read_s_python() {
        let lines = super::super::shell_python("inbox.sh", "cmd_inbox_read() {");
        let line = |n: usize| lines[n - 1].trim().to_string();
        assert_eq!(line(3), "raw = sys.stdin.read()");
        assert_eq!(line(4), "if not raw.strip():");
        assert_eq!(
            line(5),
            format!(
                "sys.stderr.write('{}\\n')",
                READ_NO_ANSWER
                    .replace(k2_client::brand::PRODUCT_NAME, "K2")
                    .replace('\'', "\\'")
            )
        );
        assert_eq!(
            line(10),
            format!(
                "sys.stderr.write('{}%r\\n' % raw[:200])",
                READ_UNUSABLE.replace(k2_client::brand::PRODUCT_NAME, "K2")
            )
        );
        assert_eq!(
            line(12),
            "err = d.get('error') if isinstance(d, dict) else 'unexpected answer'"
        );
        assert_eq!(line(14), "hint = (err.get('hint') or err.get('code') or str(err)) if isinstance(err, dict) else str(err)");
        assert_eq!(line(15), "sys.stderr.write('Error: %s\\n' % hint)");
        assert_eq!(
            line(PY_READ_LOWER as usize),
            "sys.exit(4 if 'not found' in hint.lower() or (isinstance(err, dict) and err.get('code') == 'not_found') else 1)"
        );
        assert_eq!(line(17), "print(d.get('content', ''))");
        assert_eq!(lines.len(), 17);
        let folders = super::super::shell_python("inbox.sh", "cmd_inbox_folders() {");
        assert_eq!(
            folders[3].trim(),
            "folders = json.loads(sys.stdin.read() or '[]')"
        );
        assert_eq!(folders[5].trim(), "folders = []");
    }

    fn list(folder: &str) -> Option<Ask> {
        Some(Ask::List {
            folder: folder.into(),
        })
    }

    #[test]
    fn the_words() {
        assert_eq!(parse(&[]), list(""));
        assert_eq!(parse(&["list"]), list(""));
        assert_eq!(parse(&["list", "--folder", "a b"]), list("a b"));
        assert_eq!(parse(&["list", "projects", "other"]), list("projects"));
        assert_eq!(parse(&["list", "--json", "x"]), list("x"));
        assert_eq!(parse(&["list", "x", "--folder", "y"]), list("y"));
        assert_eq!(parse(&["list", "--folder", "", "z"]), list("z"));
        assert_eq!(parse(&["list", "--folder", "-x"]), list("-x"));
        assert_eq!(parse(&["list", "-"]), list(""));
        let needs = |f: &str| Some(Ask::NeedsValue(f.into()));
        assert_eq!(parse(&["list", "--folder"]), needs("--folder"));
        assert_eq!(parse(&["--folder"]), needs("--folder"));
        assert_eq!(parse(&["x", "--folder"]), needs("--folder"));
        // A first word that is no verb is list's, itself included.
        assert_eq!(parse(&["--json", "zz"]), list("zz"));
        assert_eq!(parse(&["projects"]), list("projects"));
        assert_eq!(parse(&["help"]), list("help"));
        assert_eq!(parse(&[""]), list(""));
        assert_eq!(parse(&["", "a"]), list("a"));
        assert_eq!(parse(&["--folder", "f"]), list("f"));
        assert_eq!(parse(&["read"]), Some(Ask::Read { id: String::new() }));
        assert_eq!(
            parse(&["read", "a1", "extra"]),
            Some(Ask::Read { id: "a1".into() })
        );
        assert_eq!(
            parse(&["read", "--x"]),
            Some(Ask::Read { id: "--x".into() })
        );
        assert_eq!(parse(&["search"]), Some(Ask::Search { q: String::new() }));
        assert_eq!(
            parse(&["search", "q &x"]),
            Some(Ask::Search { q: "q &x".into() })
        );
        assert_eq!(parse(&["folders", "extra"]), Some(Ask::Folders));
        for help in ["--help", "-h"] {
            assert_eq!(parse(&[help, "x"]), None, "{help}");
        }
        assert_eq!(
            parse(&["compose", "--title", "a b", "x", "--body", "é&", "--from", "o"]),
            Some(Ask::Compose {
                form: Some("title=a%20b&body=%C3%A9%26&from=o".into())
            })
        );
        assert_eq!(
            parse(&["compose", "--body", "x"]),
            Some(Ask::Compose { form: None })
        );
        for f in ["title", "body", "priority", "source", "from", "summary"] {
            let flag = format!("--{f}");
            assert_eq!(parse(&["compose", "x", &flag]), needs(&flag));
        }
        assert_eq!(
            parse(&["move", "a", "f/g"]),
            Some(Ask::Write {
                route: MOVE,
                pairs: Some(vec!["id=a".into(), "folder=f%2Fg".into()]),
                usage: MOVE_USAGE,
            })
        );
        assert_eq!(
            parse(&["respond", "a"]),
            Some(Ask::Write {
                route: RESPOND,
                pairs: None,
                usage: RESPOND_USAGE,
            })
        );
        // The first bare `--` is dropped; a second one is a word.
        let respond = |words: &[&str], pairs: Option<Vec<String>>| {
            assert_eq!(
                parse(words),
                Some(Ask::Write {
                    route: RESPOND,
                    pairs,
                    usage: RESPOND_USAGE,
                }),
                "{words:?}"
            )
        };
        let both = |id: &str, text: &str| Some(vec![format!("id={id}"), format!("text={text}")]);
        respond(&["respond", "i1", "--", "--help"], both("i1", "--help"));
        respond(&["respond", "--", "-x", "--", "y"], both("-x", "--"));
        respond(&["respond", "i1", "--"], None);
        respond(&["respond", "--", "i1", "-h", "z"], both("i1", "-h"));
        assert_eq!(
            parse(&["delete"]),
            Some(Ask::Write {
                route: DELETE,
                pairs: None,
                usage: DELETE_USAGE,
            })
        );
        assert_eq!(parse(&["Read", "x"]), list("Read"));
    }

    #[test]
    fn format_inbox_prints_what_python_does() {
        let f = |s: &str| format_inbox(s.as_bytes());
        for none in ["", "[]", "{}", "null", "false", "0", "0.0", "\"\""] {
            assert_eq!(f(none), "No items.\n", "{none}");
        }
        // Not JSON, or not a list: nothing (its errors are thrown away).
        for nothing in [
            "hello",
            " ",
            "\"abc\"",
            "5",
            "true",
            r#"{"error":{"code":"x"}}"#,
            "[1]",
            r#"["a"]"#,
            "\u{feff}[]",
        ] {
            assert_eq!(f(nothing), "", "{nothing}");
        }
        assert_eq!(format_inbox(b"\xff"), "");
        assert_eq!(
            f(r#"[{"id":"a1","folder":"projects","priority":"high","title":"T","summary":"S"}]"#),
            format!(
                "  a1{} [projects  ] ! T\n  {}   summary: S\n",
                " ".repeat(22),
                " ".repeat(24)
            )
        );
        assert_eq!(
            f(r#"[{"id":"b","filename":"f.md","priority":"low"}]"#),
            format!("  b{} [(top)     ] . f.md\n", " ".repeat(23))
        );
        assert_eq!(
            f(r#"[{"id":5,"folder":7,"priority":"critical","title":null}]"#),
            format!("  5{} [7         ] !! \n", " ".repeat(23))
        );
        assert_eq!(
            f(r#"[{"id":true,"folder":1.5}]"#),
            format!("  1{} [1.5       ]   \n", " ".repeat(23))
        );
        assert_eq!(
            f(r#"[{}]"#),
            format!("  ?{} [(top)     ]   \n", " ".repeat(23))
        );
        // Each at its own place: a long id is not cut.
        let long = "x".repeat(30);
        assert_eq!(
            f(&format!(
                r#"[{{"id":"{long}","folder":"élan","title":"é","priority":1}}]"#
            )),
            format!("  {long} [élan      ]   é\n")
        );
        // A title that is falsy gives way to the filename; a summary that is falsy is no line.
        assert_eq!(
            f(r#"[{"id":"i","title":"","filename":"f","summary":""}]"#),
            format!("  i{} [(top)     ]   f\n", " ".repeat(23))
        );
        assert_eq!(
            f(r#"[{"id":"i","title":[1,"a"],"summary":{"k":null}}]"#)
                .lines()
                .last(),
            Some(format!("  {}   summary: {{'k': None}}", " ".repeat(24)).as_str())
        );
        // Where Python raises, what came before stays.
        assert_eq!(
            f(r#"[{"id":"a"},{"id":null}]"#),
            format!("  a{} [(top)     ]   \n", " ".repeat(23))
        );
        assert_eq!(
            f(r#"[{"id":"a"},{"id":"b","folder":[1]}]"#).lines().count(),
            1
        );
        assert_eq!(
            f(r#"[{"id":"a"},{"id":"b","priority":["x"]}]"#)
                .lines()
                .count(),
            1
        );
        assert_eq!(
            f(r#"[{"id":"a"},{"id":"b","priority":{}}]"#)
                .lines()
                .count(),
            1,
            "an empty dict is unhashable too"
        );
        assert_eq!(f(r#"[{"id":"a"},"b",{"id":"c"}]"#).lines().count(), 1);
        assert_eq!(
            f(r#"[{"id":"a","summary":"\ud83d"},{"id":"c"}]"#)
                .lines()
                .count(),
            1,
            "the summary can't be printed"
        );
        assert_eq!(f(r#"[{"id":"\ud83d"}]"#), "");
        assert_eq!(
            f(r#"[{"id":"a","folder":"","priority":null,"title":0,"filename":"f"}]"#),
            format!("  a{} [(top)     ]   f\n", " ".repeat(23))
        );
    }

    #[test]
    fn folders_prints_what_python_does() {
        let f = |s: &str| folders(s.as_bytes());
        let none = Shown::out(
            "No folders yet. Create one by moving an item:\n  k2 inbox move <id> projects\n",
        );
        for empty in ["", "[]", "not json", "{}", "null", "0"] {
            assert_eq!(f(empty), none, "{empty}");
        }
        assert_eq!(folders(b"\xff"), none);
        assert_eq!(f(r#"["a","b é"]"#), Shown::out("  a\n  b é\n"));
        assert_eq!(
            f(r#"["a",1,"c"]"#),
            Shown {
                stdout: "  a\n".into(),
                stderr: String::new(),
                exit: Exit::Error
            }
        );
        assert_eq!(f(r#""ab""#), Shown::out("  a\n  b\n"));
        assert_eq!(f(r#"{"x":1,"y":2}"#), Shown::out("  x\n  y\n"));
        assert_eq!(f("5"), Shown::err("", Exit::Error));
        assert_eq!(f("true"), Shown::err("", Exit::Error));
        assert_eq!(
            f(r#"["a","\ud83d"]"#),
            Shown {
                stdout: "  a\n".into(),
                stderr: String::new(),
                exit: Exit::Error
            }
        );
    }

    #[test]
    fn read_item_prints_what_python_does() {
        let f = |s: &str| read_item(s.as_bytes());
        let no_answer = Shown::err(format!("{READ_NO_ANSWER}\n"), Exit::Error);
        for empty in ["", " \n\t", "\u{a0}\u{3000}"] {
            assert_eq!(f(empty), no_answer, "{empty:?}");
        }
        assert_eq!(
            f(r#"{"content":"hello\nworld"}"#),
            Shown::out("hello\nworld\n")
        );
        assert_eq!(f(r#"{"content":5}"#), Shown::out("5\n"));
        assert_eq!(f(r#"{"x":1}"#), Shown::out("\n"));
        assert_eq!(f(r#"{"error":null,"content":"c"}"#), Shown::out("c\n"));
        assert_eq!(
            f("x\n"),
            Shown::err(format!("{READ_UNUSABLE}'x\\n'\n"), Exit::Error)
        );
        assert_eq!(
            f("it's"),
            Shown::err(format!("{READ_UNUSABLE}\"it's\"\n"), Exit::Error)
        );
        let long = "y".repeat(250);
        assert_eq!(
            f(&long),
            Shown::err(
                format!("{READ_UNUSABLE}'{}'\n", "y".repeat(200)),
                Exit::Error
            )
        );
        assert_eq!(
            f("[1]"),
            Shown::err("Error: unexpected answer\n", Exit::Error)
        );
        assert_eq!(
            f("\"x\""),
            Shown::err("Error: unexpected answer\n", Exit::Error)
        );
        assert_eq!(
            f(r#"{"error":"inbox item not found: zzz"}"#),
            Shown::err("Error: inbox item not found: zzz\n", Exit::NotFound)
        );
        assert_eq!(
            f(r#"{"error":"Item NOT FOUND"}"#),
            Shown::err("Error: Item NOT FOUND\n", Exit::NotFound)
        );
        assert_eq!(
            f(r#"{"error":{"code":"not_found"}}"#),
            Shown::err("Error: not_found\n", Exit::NotFound)
        );
        assert_eq!(
            f(r#"{"error":{"code":"not_found","hint":"gone"}}"#),
            Shown::err("Error: gone\n", Exit::NotFound)
        );
        assert_eq!(
            f(r#"{"error":{"code":"owner_only","hint":"Only the owner can do that."}}"#),
            Shown::err("Error: Only the owner can do that.\n", Exit::Error)
        );
        assert_eq!(
            f(r#"{"error":{"hint":"","code":""}}"#),
            Shown::err("Error: {'hint': '', 'code': ''}\n", Exit::Error)
        );
        assert_eq!(f(r#"{"error":{}}"#), Shown::err("Error: {}\n", Exit::Error));
        assert_eq!(
            f(r#"{"error":false}"#),
            Shown::err("Error: False\n", Exit::Error)
        );
        assert_eq!(
            f(r#"{"error":[1]}"#),
            Shown::err("Error: [1]\n", Exit::Error)
        );
        assert_eq!(
            f(r#"{"error":{"hint":5}}"#),
            Shown::err(
                "Error: 5\nTraceback (most recent call last):\n  File \"<string>\", line 16, in <module>\nAttributeError: 'int' object has no attribute 'lower'\n",
                Exit::Error
            )
        );
        assert!(f(r#"{"error":{"code":["x"]}}"#)
            .stderr
            .ends_with("AttributeError: 'list' object has no attribute 'lower'\n"));
    }
}
