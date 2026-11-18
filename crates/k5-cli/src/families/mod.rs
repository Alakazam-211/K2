//! The command families the binary answers itself, one module each. Each
//! does what the shell CLI's command does, byte for byte: the same
//! requests, the same output and the same exit code.
//!
//! - [`whoami`]: `k5 whoami [--json]`.
//! - [`done`]: `k5 done [--blocked "reason"] [--agent <name>]`.
//! - [`respond`]: `k5 respond [--final] <message>`.
//! - [`inbox`]: `k5 inbox` and its read forms (`list`, `read`, `search`,
//!   `folders`).
//! - [`read`]: `k5 read <workspace>`, a workspace's live terminal.
//! - [`activity`]: `k5 activity`, the activity feed.
//!
//! A family may hand a command line back to the shell CLI ([`done::run`]
//! and the others return `None`) when it is a form only the shell
//! answers. It decides before it prints or sends anything.
//!
//! # A request on the left of a pipe
//!
//! `inbox`, `read` and `activity` send `cli_send cli_request … | python3 -c …`
//! (`activity`: `cli_request`, over TCP only, [`request_piped_tcp`]), under
//! `set -euo pipefail` ([`request_piped`]). The reader on the right reads
//! what the call printed: the answer's bytes with the NULs dropped, and
//! with its trailing newlines dropped too when the cell socket answered
//! (`cli_send` holds that answer in `$(…)`); nothing when the call
//! failed. The command ends with the reader's code when it is not 0,
//! else the call's (curl's code, or 75). A request with no answer is the
//! transport's line, then `_k2_end_command` from inside the pipe: the
//! reader is stopped before it reads anything and the main shell gets
//! SIGPIPE, so the command dies of SIGPIPE (exit status 141 to a shell)
//! with no EXIT trap lines ([`end_in_pipe`]).

use crate::exit::Exit;
use crate::out::json::{self, Text, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::{Call, Channel, Client, Error, Response};
use std::path::Path;

pub mod activity;
pub mod done;
pub mod inbox;
pub mod read;
pub mod respond;
pub mod whoami;

/// The port as curl takes it from the shell's URL: digits only, and a
/// port number.
pub fn curl_port(text: &str) -> Option<u16> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

/// What `printf '%s' "$out"` prints of a body the shell read into `out`
/// with `$(…)` and a sentinel: every byte but the NULs.
pub fn without_nuls(body: &[u8]) -> Vec<u8> {
    body.iter().copied().filter(|b| *b != 0).collect()
}

/// What the shell's Python prints: stdout, stderr and its exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub stdout: String,
    pub stderr: String,
    pub exit: Exit,
}

impl Shown {
    /// Only `stdout`, and exit 0.
    pub fn out(stdout: impl Into<String>) -> Shown {
        Shown {
            stdout: stdout.into(),
            stderr: String::new(),
            exit: Exit::Ok,
        }
    }

    /// Only `stderr`, and `exit`.
    pub fn err(stderr: impl Into<String>, exit: Exit) -> Shown {
        Shown {
            stdout: String::new(),
            stderr: stderr.into(),
            exit,
        }
    }

    /// Writes stdout, then stderr; the exit. A broken stdout ends as
    /// [`out::after_stdout_error`] says.
    pub fn print(&self) -> Exit {
        if !self.stdout.is_empty() {
            if let Err(e) = out::stdout(&self.stdout) {
                return out::after_stdout_error(&e);
            }
        }
        if !self.stderr.is_empty() {
            let _ = out::write_flushed(&mut std::io::stderr().lock(), self.stderr.as_bytes());
        }
        self.exit
    }
}

/// A Python traceback as `python3 -c` prints it (Python 3.12: no source
/// lines for `<string>`): each frame's line and name, then the exception.
pub fn traceback(frames: &[(u32, &str)], exception: &str) -> String {
    let mut s = String::from("Traceback (most recent call last):\n");
    for (line, name) in frames {
        s.push_str(&format!("  File \"<string>\", line {line}, in {name}\n"));
    }
    s.push_str(exception);
    s.push('\n');
    s
}

/// `d.get(key)` on a JSON object's pairs.
pub fn get<'a>(pairs: &'a [(Text, Value)], key: &str) -> Option<&'a Value> {
    let key = Text::from(key);
    pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| v)
}

/// What the reader on the right of the pipe reads of an answer: the bytes
/// `printf '%s'` hands on, NULs dropped, and the trailing newlines too
/// when it came over the cell socket (`cli_send` held it in `$(…)`).
pub fn piped_body(r: &Response) -> Vec<u8> {
    let mut body = without_nuls(&r.body);
    if r.channel == Channel::Uds {
        while body.last() == Some(&b'\n') {
            body.pop();
        }
    }
    body
}

/// The left side of the pipe: what the reader reads, and the code the
/// call ends with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piped {
    pub body: Vec<u8>,
    pub exit: Exit,
}

/// `cli_send cli_request <endpoint> <args>` on the left of a pipe (see the
/// module's notes). Each answer goes through `closing`. A request with no
/// answer never comes back: its line, then [`end_in_pipe`].
pub fn request_piped(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
) -> Piped {
    let r = client.send(endpoint, Call::Request(args));
    piped(client, closing, r)
}

/// `cli_request <endpoint> <args>` on the left of a pipe: as
/// [`request_piped`], over TCP only.
pub fn request_piped_tcp(
    client: &mut Client,
    closing: &mut Closing,
    endpoint: &str,
    args: &[&str],
) -> Piped {
    let r = client.call(Channel::Tcp, endpoint, Call::Request(args));
    piped(client, closing, r)
}

fn piped(client: &Client, closing: &mut Closing, result: Result<Response, Error>) -> Piped {
    let passport = client.passport().is_some();
    match result {
        Ok(r) => {
            closing.answer(&r, passport);
            Piped {
                body: piped_body(&r),
                exit: Exit::Ok,
            }
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            end_in_pipe()
        }
        Err(e) => Piped {
            body: Vec::new(),
            exit: Exit::from_curl(e.curl_code()),
        },
    }
}

/// `_k2_end_command` from the left of a pipe: the command dies of SIGPIPE,
/// with no EXIT trap lines. Where the signal can't end the process (it is
/// blocked, or there is none), exit 1.
pub fn end_in_pipe() -> ! {
    out::die_of_sigpipe();
    std::process::exit(Exit::Error.code())
}

/// `set -o pipefail`: the reader's code when it is not 0, else the call's.
pub fn pipefail(call: Exit, reader: Exit) -> Exit {
    if reader == Exit::Ok {
        call
    } else {
        reader
    }
}

/// `command -v python3` finds one: the shell's readers need it. Without
/// one the shell prints the answer as it came (or Python's "command not
/// found"), so a family hands such a line to the shell.
pub fn has_python3() -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| crate::daemon::find_on_path("python3", path).is_some())
}

/// The `HOOK_SOCK` setting (its K5_, K2_ and K2SO_ names, empty counts as
/// unset).
pub fn hook_sock() -> Option<std::path::PathBuf> {
    k2_client::env_names::first_nonempty_os_in(k2_client::env_names::HOOK_SOCK, |n| {
        std::env::var_os(n)
    })
    .map(|(_, v)| v.into())
}

/// `_cli_use_passport_project` (`cli/src/cell-socket.sh`): with a passport,
/// no `PROJECT_PATH` setting and no remote host, ask the daemon which
/// workspace the passport belongs to (`/cli/whoami`'s `path`) and send that
/// as `project=` from here on. Nothing changes when the answer has no such
/// path (or is not JSON, or the call failed).
pub fn use_passport_project(client: &mut Client) {
    let set = k2_client::env_names::first_nonempty_os_in(k2_client::env_names::PROJECT_PATH, |n| {
        std::env::var_os(n)
    });
    if set.is_some() || client.passport().is_none() || client.force_tcp {
        return;
    }
    let out = client.passport_project();
    if out.is_empty() {
        return;
    }
    if let Some(path) = passport_path(&out) {
        client.project = path;
    }
}

/// `_cli_use_passport_project`'s Python on `out`: `path` when the answer
/// is an object whose `path` is a string with more than spaces in it,
/// spaces trimmed; else nothing. NULs go, as `$(…)` drops them.
pub fn passport_path(out: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(out).ok()?;
    let Value::Object(pairs) = json::parse(text).ok()? else {
        return None;
    };
    let Some(Value::Str(p)) = get(&pairs, "path") else {
        return None;
    };
    let p = p.as_str()?.trim_matches(py_space);
    (!p.is_empty()).then(|| p.replace('\0', ""))
}

/// Where a Python run stops with an exception: the traceback's frames
/// (line, name) and its last line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raise {
    pub frames: Vec<(u32, &'static str)>,
    pub exception: String,
}

impl Raise {
    /// An exception raised at `line` of the module.
    pub fn at(line: u32, exception: impl Into<String>) -> Raise {
        Raise {
            frames: vec![(line, "<module>")],
            exception: exception.into(),
        }
    }

    /// An exception raised at `line` of the function `name`, called from
    /// line `from` of the module.
    pub fn inside(from: u32, line: u32, name: &'static str, exception: impl Into<String>) -> Raise {
        Raise {
            frames: vec![(from, "<module>"), (line, name)],
            exception: exception.into(),
        }
    }

    /// `AttributeError: '<type>' object has no attribute '<name>'`.
    pub fn no_attribute(line: u32, v: &Value, name: &str) -> Raise {
        Raise::at(
            line,
            format!(
                "AttributeError: '{}' object has no attribute '{name}'",
                py::type_name(v)
            ),
        )
    }

    /// The traceback's text, as `python3 -c` prints it (no source lines).
    pub fn text(&self) -> String {
        traceback(&self.frames, &self.exception)
    }
}

/// What a Python reader printed before it raised, and the traceback: its
/// exit is 1.
pub fn raised(stdout: String, stderr: String, r: &Raise) -> Shown {
    Shown {
        stdout,
        stderr: stderr + &r.text(),
        exit: Exit::Error,
    }
}

/// `v.get(key)` on a value Python must find a dict: `None` for a missing
/// key, the `AttributeError` at `line` for anything but a dict.
pub fn py_get<'a>(v: &'a Value, key: &str, line: u32) -> Result<Option<&'a Value>, Raise> {
    match v {
        Value::Object(pairs) => Ok(get(pairs, key)),
        other => Err(Raise::no_attribute(line, other, "get")),
    }
}

/// `for x in v`: the items Python walks (a string's code points, a
/// dict's keys), or the `TypeError` at `line`.
pub fn py_iter(v: &Value, line: u32) -> Result<Vec<Value>, Raise> {
    match v {
        Value::Array(items) => Ok(items.clone()),
        Value::Str(t) => Ok(t
            .code_points()
            .into_iter()
            .map(|cp| Value::Str(code_point(cp)))
            .collect()),
        Value::Object(pairs) => Ok(pairs.iter().map(|(k, _)| Value::Str(k.clone())).collect()),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: '{}' object is not iterable",
                py::type_name(other)
            ),
        )),
    }
}

/// `len(v)`, or the `TypeError` at `line`.
pub fn py_len(v: &Value, line: u32) -> Result<usize, Raise> {
    match v {
        Value::Array(items) => Ok(items.len()),
        Value::Str(t) => Ok(t.code_points().len()),
        Value::Object(pairs) => Ok(pairs.len()),
        other => Err(Raise::at(
            line,
            format!(
                "TypeError: object of type '{}' has no len()",
                py::type_name(other)
            ),
        )),
    }
}

/// `'%d' % v`, or the exception at `line`.
pub fn py_d(v: &Value, line: u32) -> Result<String, Raise> {
    py::percent_d(v).map_err(|e| Raise::at(line, e))
}

/// `'%-<width>s' % v`: `str(v)` padded with spaces to `width` code points.
pub fn py_left(v: &Value, width: usize) -> String {
    ljust(&py::str_of(v), width)
}

/// `s.ljust(width)`: padded with spaces to `width` code points.
pub fn ljust(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n < width {
        format!("{s}{}", " ".repeat(width - n))
    } else {
        s.to_string()
    }
}

/// A one-code-point string, half of a surrogate pair too.
pub fn code_point(cp: u32) -> Text {
    match char::from_u32(cp) {
        Some(c) => Text::from(c.to_string()),
        None => match json::parse(&format!("\"\\u{cp:04x}\"")) {
            Ok(Value::Str(t)) => t,
            _ => Text::from("\u{fffd}"),
        },
    }
}

/// A string value.
pub fn s(text: &str) -> Value {
    Value::Str(Text::from(text))
}

/// `[ -S "$sock" ]`.
pub fn live_socket(sock: Option<&Path>) -> bool {
    sock.is_some_and(crate::transport::client::is_socket)
}

/// Python's `str.isspace()` for one code point: what `str.strip()` takes
/// off.
pub fn py_space(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n'
            | '\u{0b}'
            | '\u{0c}'
            | '\r'
            | '\u{1c}'..='\u{1f}'
            | ' '
            | '\u{85}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
    )
}

/// The shell's Python as `python3 -c` numbers its lines: the text between
/// the `-c "` that follows `after` in `file` (under `cli/src`) and the
/// next line that is only `"`. Line 1 is the empty one after `-c "`.
#[cfg(test)]
pub fn shell_python(file: &str, after: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../cli/src")
        .join(file);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let body = text
        .split_once(after)
        .unwrap_or_else(|| panic!("no {after:?} in {file}"))
        .1;
    let py = &body[body
        .find("python3 -c \"")
        .unwrap_or_else(|| panic!("no python3 -c after {after:?}"))
        + "python3 -c \"".len()..];
    let end = py.find("\n\"").expect("the end of the Python");
    py[..end].split('\n').map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_as_curl_reads_it() {
        assert_eq!(curl_port("4100"), Some(4100));
        for bad in ["", "41o0", " 4100", "4100 ", "-1", "65536", "+1"] {
            assert_eq!(curl_port(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn nuls_go_and_nothing_else() {
        assert_eq!(without_nuls(b"a\0b\n\n"), b"ab\n\n");
        assert_eq!(without_nuls(b""), b"");
    }

    fn answer(channel: Channel, body: &[u8]) -> Response {
        Response {
            status: 200,
            body: body.to_vec(),
            channel,
            permission: None,
            server_version: None,
        }
    }

    #[test]
    fn what_the_reader_reads() {
        assert_eq!(piped_body(&answer(Channel::Tcp, b"x\0y\n\n")), b"xy\n\n");
        assert_eq!(piped_body(&answer(Channel::Uds, b"x\0y\n\n")), b"xy");
        assert_eq!(piped_body(&answer(Channel::Uds, b"\n \n")), b"\n ");
    }

    #[test]
    fn pipefail_keeps_the_reader_s_code() {
        assert_eq!(pipefail(Exit::Curl(56), Exit::Ok), Exit::Curl(56));
        assert_eq!(pipefail(Exit::DaemonUnreachable, Exit::Error), Exit::Error);
        assert_eq!(pipefail(Exit::Ok, Exit::NotFound), Exit::NotFound);
        assert_eq!(pipefail(Exit::Ok, Exit::Ok), Exit::Ok);
    }

    #[test]
    fn python_s_spaces() {
        let s = " \t\n\u{0b}\u{0c}\r\u{1c}\u{1f}\u{85}\u{a0}\u{1680}\u{2000}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}";
        assert!(s.chars().all(py_space));
        for c in ['x', '\u{0}', '\u{1b}', '\u{200b}', '\u{feff}', '\u{180e}'] {
            assert!(!py_space(c), "{c:?}");
        }
    }
}
