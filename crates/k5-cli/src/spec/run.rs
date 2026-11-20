//! The spec runner: one [`SpecVerb`] row, run as its bash verb runs it.
//!
//! What runs, in the shell's order (`cli/src/prelude.sh`, then the verb):
//!
//! 1. The settings ([`crate::daemon`]) and the two gates
//!    ([`crate::prelude`]).
//! 2. The tool's checks with `HOOK_SOCK` set: the stale-passport and
//!    require-passport lines, exit 3 ([`crate::prelude::id_tool`]). The
//!    runner keeps them only because bash makes them before the verb
//!    runs (K2's ruling); they go with [`crate::tools`] after T5. A row's
//!    tool is always an `id` tool: the runner hands any other to the shell,
//!    whose open tools send the owner token over TCP in a K2 terminal
//!    (COMPAT-58) where the binary sends the passport (PRD 13.9 #4).
//! 3. The words, by the row's style ([`parse`]): the usage line and exit
//!    2 on a word the verb doesn't take, before anything is sent.
//! 4. One request through the row's via ([`Via::Compute`]: `_compute_call`),
//!    with the row's credential ([`Cred`]: the kind, never a decision of
//!    its own: what the daemon refuses is its answer).
//! 5. On any status but 200 the via's failure line on stderr and its exit
//!    ([`fail_line`], [`exit_for`]); the refusal's "Ask with:" line comes
//!    from the EXIT trap ([`Closing`]), the exit stays the via's.
//! 6. On 200, the row's output ([`Print`]), then the EXIT trap's lines.
//!
//! It hands a command line to the shell ([`run`] returns `None`) before
//! anything is printed or sent when: a word is not UTF-8, `-h` or `--help`
//! is in it, `HOOK_SOCK` is set with a `CODEX_` variable exported, the
//! row's tool is not an `id` tool, the binary can't tell what the shell's
//! `python3` is, the port is set but is no port curl takes, or the
//! environment sets `_CP_404_EXIT` (which `_compute_fail` would read).
//!
//! Where this differs from the shell: an answer whose bytes are not UTF-8
//! is read with each bad byte as U+FFFD (Python's stdin depends on the
//! locale there), and a transfer that failed after part of an answer came
//! is read as one with no answer (curl's `-w` would follow the part).

use super::{Cred, Enc, Method, Print, Source, SpecVerb, Style, Via};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{self, curl_port, get, py_get, raised, without_nuls, Raise, Shown};
use crate::out::json::{self, Options, Text, Value};
use crate::out::{self, py};
use crate::prelude::{self, Closing};
use crate::transport::{permission, Channel, Client, Error, Response};
use crate::{passport, python, tools};
use k2_client::brand::PRODUCT_NAME;
use std::ffi::OsString;
use std::path::Path;

/// The line `_compute_text` prints for a 200 with no `text`.
pub const NO_TEXT: &str = concat!(
    "(this server sent no text: ",
    k2_client::cli_name!(),
    " update)"
);

/// What the words ask for: the JSON form, and each positional word's
/// value by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ask {
    pub json: bool,
    pub values: Vec<(&'static str, String)>,
}

/// The words after the row's own (`words`: the whole command line after
/// `argv[0]`), by the row's style. `Err`: the usage line, exit 2.
pub fn parse(row: &SpecVerb, words: &[&str]) -> Result<Ask, &'static str> {
    let rest = &words[row.words.len().min(words.len())..];
    let usage = row.parse.usage;
    match row.parse.style {
        Style::None => Ok(Ask {
            json: row.parse.json_flag && rest.contains(&"--json"),
            values: Vec::new(),
        }),
        Style::For => {
            let arg = row.parse.args.first().ok_or(usage)?;
            let (mut json, mut value) = (false, String::new());
            for w in rest {
                match *w {
                    "--json" if row.parse.json_flag => json = true,
                    w if w.starts_with('-') => return Err(usage),
                    w => {
                        // `[ -z "$job" ] || usage`: an empty word before
                        // leaves room for the next.
                        if !value.is_empty() {
                            return Err(usage);
                        }
                        value = w.to_string();
                    }
                }
            }
            if !arg.check.is_some_and(|c| c.passes(&value)) {
                return Err(usage);
            }
            Ok(Ask {
                json,
                values: vec![(arg.name, value)],
            })
        }
    }
}

/// The query: each item's `key=value`, joined by `&`, an item with an
/// empty value left out.
pub fn query(row: &SpecVerb, ask: &Ask) -> String {
    let mut parts = Vec::new();
    for item in row.call.query {
        let value = match item.from {
            Source::Literal(v) => v.to_string(),
            Source::Arg(name) => ask
                .values
                .iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default(),
        };
        if value.is_empty() {
            continue;
        }
        let value = match item.enc {
            Enc::Raw => value,
            Enc::Quote => crate::transport::encode::quote(&value),
        };
        parts.push(format!("{}={value}", item.key));
    }
    parts.join("&")
}

/// The row's tool in the shell's policy, when the runner may answer it:
/// an `id` tool, the same for the noun alone (`_cli_check_session_passport
/// "$1"`) and with its verb (`_cli_apply_auth_for_verb "$1" "$2"`).
pub fn id_tool_of(row: &SpecVerb) -> Option<&'static str> {
    let (noun, verb) = (row.words[0], row.words.get(1).copied().unwrap_or(""));
    let tool = tools::tool_id(noun, verb);
    (tools::is_id(tool) && tools::tool_id(noun, "") == tool).then_some(tool)
}

/// Runs `row` for `args` (the whole command line after `argv[0]`). `None`:
/// the shell CLI answers it (see the module's notes).
pub fn run(row: &'static SpecVerb, args: &[OsString]) -> Option<Exit> {
    let hook_sock = families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    if words.len() < row.words.len()
        || words[..row.words.len()] != *row.words
        || words.iter().any(|w| matches!(*w, "-h" | "--help"))
    {
        return None;
    }
    let tool = id_tool_of(row)?;
    match row.call.via {
        Via::Compute => {
            if std::env::var_os("_CP_404_EXIT").is_some_and(|v| !v.is_empty()) {
                return None;
            }
        }
    }
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
    let ask = parse(row, &words);
    Some(run_with(
        row,
        args,
        tool,
        ask,
        hook_sock.as_deref(),
        settings,
    ))
}

fn run_with(
    row: &SpecVerb,
    args: &[OsString],
    tool: &str,
    ask: Result<Ask, &'static str>,
    hook_sock: Option<&Path>,
    mut settings: Settings,
) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, tool) {
        return stop;
    }
    let ask = match ask {
        Ok(a) => a,
        Err(usage) => {
            out::stderr_line(usage);
            return Exit::Usage;
        }
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
    match row.auth {
        // The settings' credential: the passport, else the owner token.
        Cred::Session => {}
    }
    let mut closing = Closing::default();
    let exit = match call(row, &mut client, &ask) {
        Err(line) => {
            out::stderr_line(&line);
            Exit::Error
        }
        Ok((status, body)) => {
            let r = Response {
                status: status.parse().unwrap_or(0),
                permission: permission::note(&body),
                body: body.clone(),
                channel: Channel::Tcp,
                server_version: None,
            };
            closing.answer(&r, client.passport().is_some());
            if status != "200" {
                out::stderr_line(&fail_line(&status, &body));
                exit_for(row, &status)
            } else {
                let print = if ask.json {
                    row.output.json.unwrap_or(row.output.text)
                } else {
                    row.output.text
                };
                show(print, &body)
            }
        }
    };
    closing.flush(&mut client, exit);
    exit
}

/// The via's request: `_compute_call`'s `raw="$(_compute_send …)"` and
/// `_compute_split`, as the status's three digits and the body. `Err`: a
/// POST the cell socket may have run, or a session with no passport whose
/// TCP request would carry the owner token (`prelude::id_tool` stops that
/// one first): its line, and the command ends with 1.
fn call(row: &SpecVerb, client: &mut Client, ask: &Ask) -> Result<(String, Vec<u8>), String> {
    let payload = query(row, ask);
    let post = match row.call.method {
        Method::Get => false,
    };
    let r = match row.call.via {
        Via::Compute => client.compute_send(post, row.call.path, &payload, row.call.tcp),
    };
    if let Err(e @ (Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. })) = &r {
        return Err(e.to_string());
    }
    Ok(split(&raw(&r, client.refused_before_answer())))
}

/// What `raw="$(_compute_send …)"` holds: curl's `-w` line for each try
/// that nothing accepted (`refused`), then the answer and its status, or,
/// when the call failed, one more `-w` line and `_compute_send`'s own
/// `printf '\n000'`. `$(…)` drops the NULs; the status's digits end it, so
/// no newline is dropped.
pub fn raw(r: &Result<Response, Error>, refused: u32) -> Vec<u8> {
    let mut out = b"\n000".repeat(refused as usize);
    match r {
        Ok(r) => {
            out.extend(without_nuls(&r.body));
            out.extend(format!("\n{:03}", r.status).into_bytes());
        }
        Err(_) => out.extend(b"\n000\n000"),
    }
    out
}

/// `_compute_split`: the status after the last newline (`000` when
/// empty), the body before it (nothing when there is no newline).
pub fn split(raw: &[u8]) -> (String, Vec<u8>) {
    let (status, body) = match raw.iter().rposition(|b| *b == b'\n') {
        Some(i) => (&raw[i + 1..], raw[..i].to_vec()),
        None => (raw, Vec::new()),
    };
    let status = String::from_utf8_lossy(status).into_owned();
    if status.is_empty() {
        ("000".into(), body)
    } else {
        (status, body)
    }
}

/// `_compute_fail`'s line (not an App area: no refusal registry rows):
/// its Python's on the body (one JSON object of `error` and `hint`, or
/// `daemon_error: ` and the first 200 characters of a body that is not
/// JSON), `$(…)` dropping the trailing newlines; the no-answer object when
/// that is empty.
pub fn fail_line(status: &str, body: &[u8]) -> String {
    let mut line = String::new();
    if !body.is_empty() {
        let raw = String::from_utf8_lossy(body);
        line = match json::parse(&raw) {
            Err(_) => format!(
                "daemon_error: {}",
                raw.chars().take(200).collect::<String>()
            ),
            Ok(v) => {
                let e = match &v {
                    Value::Object(pairs) => get(pairs, "error"),
                    _ => None,
                };
                let message = match &v {
                    Value::Object(pairs) => get(pairs, "message"),
                    _ => None,
                };
                let s = |t: &str| Value::Str(Text::from(t));
                let (error, hint) = match (e, message) {
                    (Some(Value::Object(p)), _) => (
                        get(p, "code").cloned().unwrap_or_else(|| s("error")),
                        get(p, "hint").cloned().unwrap_or_else(|| s("")),
                    ),
                    (Some(e @ Value::Str(_)), Some(m @ Value::Str(_))) => (e.clone(), m.clone()),
                    (e, _) => (s("error"), s(&py::str_of(e.unwrap_or(&Value::Null)))),
                };
                let out = Value::Object(vec![
                    (Text::from("error"), error),
                    (Text::from("hint"), hint),
                ]);
                json::to_string(&out, &Options::default())
                    .expect("json.dumps with ensure_ascii writes any value json.loads gave")
            }
        };
        while line.ends_with('\n') {
            line.pop();
        }
    }
    if line.is_empty() {
        line = format!(
            "{{\"error\":\"daemon_unreachable\",\"hint\":\"no answer from the {PRODUCT_NAME} daemon (status {status})\"}}"
        );
    }
    line
}

/// `_compute_fail`'s exit: 400 and 413 are 2, 404 is the row's (else 3),
/// anything else 3.
pub fn exit_for(row: &SpecVerb, status: &str) -> Exit {
    match status {
        "400" | "413" => Exit::Usage,
        "404" => row
            .exits
            .iter()
            .find(|(s, _)| *s == 404)
            .map_or(Exit::Refused, |(_, code)| Exit::from_curl(*code)),
        _ => Exit::Refused,
    }
}

/// A 200's body, printed the row's way.
fn show(print: Print, body: &[u8]) -> Exit {
    match print {
        Print::Body => {
            let mut text = body.to_vec();
            text.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &text) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Print::Lines => lines(body).print(),
    }
}

/// `_compute_text`'s text form: `print("\n".join(json.load(sys.stdin)
/// .get("text") or [NO_TEXT]))`, all on the Python's line 1.
pub fn lines(body: &[u8]) -> Shown {
    let raw = String::from_utf8_lossy(body);
    let v = match json::parse(&raw) {
        Ok(v) => v,
        Err(e) => {
            let why = if raw.trim_matches(families::py_space).is_empty() {
                "Expecting value: line 1 column 1 (char 0)".to_string()
            } else {
                e.to_string()
            };
            return raised(
                String::new(),
                String::new(),
                &Raise::at(1, format!("json.decoder.JSONDecodeError: {why}")),
            );
        }
    };
    let text = match py_get(&v, "text", 1) {
        Ok(t) => t.filter(|t| py::truthy(t)).cloned(),
        Err(r) => return raised(String::new(), String::new(), &r),
    };
    let text = text.unwrap_or_else(|| Value::Array(vec![Value::Str(Text::from(NO_TEXT))]));
    let items: Vec<Value> = match &text {
        Value::Array(items) => items.clone(),
        Value::Str(_) | Value::Object(_) => match families::py_iter(&text, 1) {
            Ok(items) => items,
            Err(r) => return raised(String::new(), String::new(), &r),
        },
        _ => {
            return raised(
                String::new(),
                String::new(),
                &Raise::at(1, "TypeError: can only join an iterable"),
            )
        }
    };
    let mut out = String::new();
    for (i, item) in items.iter().enumerate() {
        let Value::Str(_) = item else {
            return raised(
                String::new(),
                String::new(),
                &Raise::at(
                    1,
                    format!(
                        "TypeError: sequence item {i}: expected str instance, {} found",
                        py::type_name(item)
                    ),
                ),
            );
        };
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&py::str_of(item));
    }
    out.push('\n');
    Shown::out(out)
}

#[cfg(test)]
mod tests;
