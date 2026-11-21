//! `k5 msg <target> --signal <kind> [--payload <json>] [--from <name>]
//! [--inbox-wake|--inbox-silent|--inbox]`: an awareness signal, as the
//! shell's `cmd_msg_signal_form` (`cli/src/msg-inbox-staging.sh`) reads
//! it and `cmd_signal` (`cli/src/awareness.sh`) sends it.
//!
//! What runs after the gates ([`super`]), in the shell's order:
//!
//! 1. The words ([`parse`]): the target is the first word, whatever it
//!    is; then `--signal`, `--payload` and `--from` each take the next
//!    word, any tray flag makes the delivery `inbox`, and every other word
//!    is skipped. A value flag as the last word is [`needs_value`]'s line
//!    and exit 2, the shell's own (`_need_value`). The sender is
//!    `--from`'s value, else `*_AGENT_NAME`, else `cli`.
//! 2. An empty target is [`usage`]'s three lines, exit 1. An empty kind
//!    is `msg`, an empty payload `{}`.
//! 3. The time (`datetime.now(timezone.utc)`, to the microsecond) and a
//!    new UUID, then the workspace's id: the project's row in K2's home
//!    database, read with the `sqlite3` on `PATH` as the shell reads it
//!    ([`workspace_id`]), else the project's path.
//! 4. The signal, as the shell's Python builds it from those values
//!    (`json.dumps`, [`body`]). Where that Python raises (a payload that
//!    is JSON but no object for `msg`, `status` or `presence`, an integer
//!    past Python's digit limit, nesting past its recursion limit), its
//!    Python 3.12 traceback and exit 1, nothing sent.
//! 5. `cli_send cli_post_json /cli/awareness/publish <signal>`: the cell
//!    socket first in a session, a POST sent once ([`Client::send`]). The
//!    answer as it came, then a newline. A failed send ends the command
//!    with curl's code; no answer, or a POST the socket may have run, is
//!    its line and exit 1.
//! 6. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers a sender in `*_AGENT_NAME` that is not UTF-8.
//!
//! Where this differs from the shell: a workspace id in the database that
//! is not UTF-8 is read with U+FFFD where the shell's Python has its
//! bytes as surrogates.

use super::super::{py_space, without_nuls, Raise};
use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::pylib::json as pyjson;
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::{Call, Channel, Client, Error};
use k2_client::cli_name;
use k2_client::env_names;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The route.
pub const ENDPOINT: &str = "/cli/awareness/publish";

/// `cmd_signal`'s lines for an empty target.
pub const USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " signal <target> <kind> <json-payload> [--inbox] [--from <sender>]\n",
    "  Default kind is 'msg'; default delivery is 'live' (interrupts running session).\n",
    "  --inbox sends as an intentional-async notice (target reads on their schedule).\n"
);

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub target: String,
    pub kind: String,
    pub payload: String,
    pub from: String,
    pub inbox: bool,
}

/// What the words come to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// The shell answers it.
    Shell,
    /// Stderr and the exit, nothing sent.
    Stop(String, Exit),
    /// The signal.
    Send(Signal),
}

/// A value flag as the last word (the shell's `_need_value` line).
pub fn needs_value(flag: &str) -> String {
    super::send::needs_value(flag)
}

/// `cmd_msg_signal_form "$1" "${@:2}"` and `cmd_signal`'s defaults, with
/// the sender `*_AGENT_NAME` names (`None` when unset or empty; `Err` when
/// not UTF-8). `words` holds `--signal` (the caller checked).
pub fn parse_with(words: &[&str], agent: Result<Option<String>, ()>) -> Parsed {
    let Some((target, rest)) = words.split_first() else {
        return Parsed::Shell;
    };
    let mut s = Signal {
        target: target.to_string(),
        kind: "msg".into(),
        payload: "{}".into(),
        from: String::new(),
        inbox: false,
    };
    let mut from = None;
    let mut i = 0;
    while i < rest.len() {
        match rest[i] {
            flag @ ("--signal" | "--payload" | "--from") => {
                let Some(v) = rest.get(i + 1) else {
                    return Parsed::Stop(needs_value(flag), Exit::Usage);
                };
                match flag {
                    "--signal" => s.kind = v.to_string(),
                    "--payload" => s.payload = v.to_string(),
                    _ => from = Some(v.to_string()),
                }
                i += 2;
            }
            "--inbox" | "--inbox-silent" | "--inbox-wake" => {
                s.inbox = true;
                i += 1;
            }
            _ => i += 1,
        }
    }
    s.from = match from {
        Some(f) => f,
        None => match agent {
            Ok(a) => a.unwrap_or_else(|| "cli".into()),
            Err(()) => return Parsed::Shell,
        },
    };
    if s.target.is_empty() {
        return Parsed::Stop(USAGE.into(), Exit::Error);
    }
    // `${2:-msg}` and `${3:-{\}}`.
    if s.kind.is_empty() {
        s.kind = "msg".into();
    }
    if s.payload.is_empty() {
        s.payload = "{}".into();
    }
    Parsed::Send(s)
}

/// [`parse_with`] on this process's `*_AGENT_NAME`.
pub fn parse(words: &[&str]) -> Parsed {
    let agent = env_names::first_nonempty_os_in(env_names::AGENT_NAME, |n| std::env::var_os(n))
        .map(|(_, v)| v.into_string().map_err(|_| ()))
        .transpose();
    parse_with(words, agent)
}

/// The signal's JSON, as the shell's Python prints it (no newline), or
/// what that Python raises.
pub fn body(s: &Signal, id: &str, workspace: &str, at: &str) -> Result<String, Raise> {
    let payload = if s.payload.chars().all(py_space) {
        Value::Object(Vec::new())
    } else {
        match pyjson::loads(&s.payload) {
            Ok(v) => v,
            Err(f) if f.class == "JSONDecodeError" => object(vec![("text", text(&s.payload))]),
            Err(f) => return Err(Raise::at(11, f.last_line())),
        }
    };
    let get = |line: u32, key: &str, default: &str| -> Result<Value, Raise> {
        match &payload {
            Value::Object(pairs) => Ok(super::super::get(pairs, key)
                .cloned()
                .unwrap_or_else(|| text(default))),
            other => Err(Raise {
                frames: vec![(15, "<module>"), (line, "<lambda>")],
                exception: format!(
                    "AttributeError: '{}' object has no attribute 'get'",
                    py::type_name(other)
                ),
            }),
        }
    };
    let kind = match s.kind.as_str() {
        "msg" => object(vec![
            ("kind", text("msg")),
            ("data", object(vec![("text", get(6, "text", "")?)])),
        ]),
        "status" => object(vec![
            ("kind", text("status")),
            ("data", object(vec![("text", get(7, "text", "")?)])),
        ]),
        "presence" => object(vec![
            ("kind", text("presence")),
            ("data", object(vec![("state", get(8, "state", "active")?)])),
        ]),
        other => object(vec![
            ("kind", text("custom")),
            (
                "data",
                object(vec![("kind", text(other)), ("payload", payload.clone())]),
            ),
        ]),
    };
    let agent = |name: &str| {
        object(vec![
            ("scope", text("agent")),
            ("workspace", text(workspace)),
            ("name", text(name)),
        ])
    };
    let signal = object(vec![
        ("id", text(id)),
        ("from", agent(&s.from)),
        ("to", agent(&s.target)),
        ("kind", kind),
        ("priority", text("normal")),
        ("delivery", text(if s.inbox { "inbox" } else { "live" })),
        ("at", text(at)),
    ]);
    // ensure_ascii writes every string, so nothing here can fail.
    Ok(json::to_string(&signal, &json::Options::default()).unwrap_or_default())
}

fn text(s: &str) -> Value {
    Value::Str(Text::from(s))
}

fn object(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (Text::from(k), v)).collect())
}

/// `datetime.now(timezone.utc).isoformat(timespec="microseconds")`.
pub fn now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    iso_utc(d.as_secs() as i64, d.subsec_micros())
}

/// Seconds and microseconds since 1970 as Python's UTC `isoformat`.
pub fn iso_utc(secs: i64, micros: u32) -> String {
    let civil =
        super::super::remote_session::civil(secs).unwrap_or_else(|| "1970-01-01 00:00:00Z".into());
    format!(
        "{}.{micros:06}+00:00",
        civil.trim_end_matches('Z').replacen(' ', "T", 1)
    )
}

/// `_k2_db_uri`: the database as a `file:` URI that never creates it.
pub fn db_uri(path: &Path) -> String {
    let p = path.to_string_lossy();
    let p = p
        .replace('%', "%25")
        .replace('?', "%3f")
        .replace('#', "%23");
    format!("file:{p}?mode=rw")
}

/// `$(sqlite3 <uri> <sql> 2>/dev/null || true)`: what it printed, its
/// trailing newlines gone.
fn sqlite(bin: &Path, db: &Path, sql: &str) -> String {
    let out = Command::new(bin)
        .arg(db_uri(db))
        .arg(sql)
        .stdin(Stdio::inherit())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    let mut text = String::from_utf8_lossy(&without_nuls(&out)).into_owned();
    while text.ends_with('\n') {
        text.pop();
    }
    text
}

/// `$(( ${n:-0} + 0 ))` on a count `sqlite3` printed.
fn count(text: &str) -> i64 {
    text.trim().parse().unwrap_or(0)
}

/// A regular file that is no symlink: `[ -f p ] && [ ! -L p ]`.
fn real_file(p: &Path) -> bool {
    p.is_file() && !p.is_symlink()
}

/// `_k2_home_db`: K2's database in `home`, as the shell picks it.
pub fn home_db(home: &Path, sqlite3: Option<&Path>) -> PathBuf {
    let new = home.join("k2.db");
    let old = home.join("k2so.db");
    if real_file(&new) && real_file(&old) {
        const LIVE: &str =
            "SELECT count(*) FROM projects WHERE id NOT IN ('_orphan', '_broadcast');";
        let (cn, co) = match sqlite3 {
            Some(bin) => (
                count(&sqlite(bin, &new, LIVE)),
                count(&sqlite(bin, &old, LIVE)),
            ),
            None => (0, 0),
        };
        if co > cn {
            return old;
        }
        if cn > co {
            return new;
        }
        let size = |p: &Path| std::fs::metadata(p).map_or(0, |m| m.len() as i64);
        let (ln, lo) = (size(&new), size(&old));
        if lo > ln * 2 && lo > 1_000_000 {
            return old;
        }
        if ln > lo * 2 && ln > 1_000_000 {
            return new;
        }
        return old;
    }
    if new.exists() || !old.exists() {
        new
    } else {
        old
    }
}

/// `cmd_signal`'s workspace id: the project's row in the home database,
/// else `project` itself.
pub fn workspace_id(home: &Path, project: &str) -> String {
    let sqlite3 = crate::families::fed::pubkey::on_path("sqlite3");
    let db = home_db(home, sqlite3.as_deref());
    if let Some(bin) = sqlite3.filter(|_| db.is_file()) {
        let sql = format!(
            "SELECT id FROM projects WHERE path = '{}' LIMIT 1;",
            project.replace('\'', "''")
        );
        let id = sqlite(&bin, &db, &sql);
        if !id.is_empty() {
            return id;
        }
    }
    project.to_string()
}

/// Steps 3 to 6 of the module's notes.
pub fn send(client: &mut Client, home: &Path, s: &Signal) -> Exit {
    let at = now();
    let id = super::send_once::uuid4();
    let project = client.project.clone();
    let workspace = workspace_id(home, &project);
    let body = match body(s, &id, &workspace, &at) {
        Ok(b) => b,
        Err(r) => {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), r.text().as_bytes());
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let passport = client.passport().is_some();
    let exit = match client.send(ENDPOINT, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            let mut shown = without_nuls(&r.body);
            if r.channel == Channel::Uds {
                while shown.last() == Some(&b'\n') {
                    shown.pop();
                }
            }
            shown.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &shown) {
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
