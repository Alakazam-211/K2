//! `k5 workspace create <path>` and `k5 workspace open <path>`, as
//! `cmd_workspace_create` and `cmd_workspace_open` (`cli/src/wiki.sh`) do
//! them: how a folder becomes a workspace.
//!
//! 1. The words ([`parse`]): `--no-wiki`, `--no-agents-md` and `--fanout`;
//!    any other word starting with `-` is `Unknown flag: <it>` and the
//!    usage line; the first other non-empty word is the path, a second one
//!    `Unexpected extra argument: <it>`; no path is the usage line. Each
//!    is exit 1, and nothing is sent.
//! 2. The path ([`path`]). `create`: `cli_abs_path`, the absolute form,
//!    the deepest folder that exists as `cd` names it and the rest as
//!    typed. `open`: a folder that exists as `cd` names it, anything else
//!    as typed. `cd` is bash's logical one ([`canonpath`]).
//! 3. One `cli_request_post /cli/workspace/<verb> path=<escaped>` with
//!    `no_wiki=1`, `no_agents_md=1` and `fanout=1` after it, in that
//!    order, for the flags given; over TCP, in `$(…)`. A POST is sent
//!    once: only a connect that was refused or timed out is tried again,
//!    a 503 `migrating` (no handler ran), an older daemon's 405 (the same
//!    call as a GET) and a refused passport healed (a 403 ran no handler).
//!    No answer is the transport's line, exit 1; any other failure is its
//!    code, with nothing printed (`set -e`).
//! 4. `cli_print_or_fail` ([`print_or_fail`]) on what `$(…)` held.
//! 5. The EXIT trap's lines.
//!
//! The shell answers a path the binary can't resolve exactly as bash
//! does: one with a newline in it, a doubled `/`, `.` or `..` that bash's
//! checks would refuse, a folder the binary may not enter, and a relative
//! path for `open`'s `cd` while `CDPATH` is set.

use super::super::{get, without_nuls};
use super::Stop;
use crate::exit::Exit;
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::prelude::Closing;
use crate::transport::encode::quote;
use crate::transport::{permission, Call, Channel, Client, Error};

/// Which verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Create,
    Open,
}

impl Verb {
    pub fn word(self) -> &'static str {
        match self {
            Verb::Create => "create",
            Verb::Open => "open",
        }
    }

    /// The route.
    pub fn endpoint(self) -> &'static str {
        match self {
            Verb::Create => "/cli/workspace/create",
            Verb::Open => "/cli/workspace/open",
        }
    }

    /// The usage line.
    pub fn usage(self) -> String {
        format!(
            "Usage: k2 workspace {} <path> [--no-wiki] [--no-agents-md] [--fanout]",
            self.word()
        )
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ask {
    pub target: String,
    pub no_wiki: bool,
    pub no_agents_md: bool,
    pub fanout: bool,
}

/// The words after the verb, as its loop reads them.
pub fn parse(verb: Verb, words: &[&str]) -> Result<Ask, Stop> {
    let stop = |lines: Vec<String>| Stop {
        lines,
        exit: Exit::Error,
    };
    let mut ask = Ask::default();
    for w in words {
        match *w {
            "--no-wiki" => ask.no_wiki = true,
            "--no-agents-md" => ask.no_agents_md = true,
            "--fanout" => ask.fanout = true,
            w if w.starts_with('-') => {
                return Err(stop(vec![format!("Unknown flag: {w}"), verb.usage()]));
            }
            w if ask.target.is_empty() => ask.target = w.to_string(),
            w => return Err(stop(vec![format!("Unexpected extra argument: {w}")])),
        }
    }
    if ask.target.is_empty() {
        return Err(stop(vec![verb.usage()]));
    }
    Ok(ask)
}

/// `[ -d "$p" ]`: `stat` follows links.
fn is_dir(p: &str) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

/// `chdir` may enter `p` (search permission on it).
#[cfg(unix)]
fn enterable(p: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(p) else {
        return false;
    };
    // SAFETY: `c` is a valid C string for the call.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn enterable(_p: &str) -> bool {
    false
}

/// bash's `sh_canonpath(path, PATH_CHECKDOTDOT | PATH_CHECKEXISTS)`, the
/// folder a logical `cd` goes to, for an absolute `path` that does not
/// start with `//`: empty parts and `.` dropped, `..` taking off the part
/// before it once what is there so far is a folder, each part kept only
/// when what is there so far is a folder. `None` where a check fails
/// (bash's `cd` then goes another way).
pub fn canonpath(path: &str) -> Option<String> {
    let joined = |parts: &[&str]| {
        if parts.is_empty() {
            "/".to_string()
        } else {
            format!("/{}", parts.join("/"))
        }
    };
    let mut parts: Vec<&str> = Vec::new();
    for el in path.split('/').filter(|e| !e.is_empty()) {
        match el {
            "." => {}
            ".." => {
                // `/..` is `/`.
                if !parts.is_empty() {
                    if !is_dir(&joined(&parts)) {
                        return None;
                    }
                    parts.pop();
                }
            }
            el => {
                parts.push(el);
                if !is_dir(&joined(&parts)) {
                    return None;
                }
            }
        }
    }
    Some(joined(&parts))
}

/// `$(cd "$dir" && pwd)` for an absolute `dir` that is a folder, when the
/// binary is sure of it: the logical name, and whether `cd` got there.
fn cd(dir: &str) -> Option<Option<String>> {
    if dir.starts_with("//") {
        return None;
    }
    let to = canonpath(dir)?;
    Some(enterable(&to).then_some(to))
}

/// `dirname` of an absolute path with no doubled `/` and no `/` at its
/// end (but `/` itself).
fn dirname(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) | None => "/",
        Some(i) => &p[..i],
    }
}

/// `basename`, likewise.
fn basename(p: &str) -> &str {
    p.rfind('/').map_or(p, |i| &p[i + 1..])
}

/// `cli_abs_path "$p"` (`cli/src/cli-tools-policy.sh`) with `pwd` as
/// `$PWD`: `None` where the binary is not sure what bash prints.
pub fn abs_path(p: &str, pwd: &str) -> Option<String> {
    if p.is_empty() {
        return Some(String::new());
    }
    let mut p = if p.starts_with('/') {
        p.to_string()
    } else {
        format!("{pwd}/{p}")
    };
    while p != "/" && p.ends_with('/') {
        p.pop();
    }
    if p.contains("//") || p.contains('\n') {
        return None;
    }
    let mut head = p.as_str();
    let mut tail = String::new();
    while !is_dir(head) {
        tail = format!("/{}{tail}", basename(head));
        head = dirname(head);
    }
    // `base="$(cd "$head" 2>/dev/null && pwd)" || base="$head"`.
    let base = cd(head)??;
    let base = if base == "/" { "" } else { base.as_str() };
    Some(format!("{base}{tail}"))
}

/// The path the verb sends for `target`, `pwd` being `$PWD` and `cdpath`
/// whether `CDPATH` is set. `None`: the shell's.
pub fn path(verb: Verb, target: &str, pwd: &str, cdpath: bool) -> Option<String> {
    if target.contains('\n') {
        return None;
    }
    match verb {
        Verb::Create => abs_path(target, pwd),
        Verb::Open => {
            if !is_dir(target) {
                return Some(target.to_string());
            }
            let absolute = if target.starts_with('/') {
                target.to_string()
            } else if cdpath {
                return None;
            } else if pwd.ends_with('/') {
                format!("{pwd}{target}")
            } else {
                format!("{pwd}/{target}")
            };
            // A `cd` that fails prints bash's own line.
            cd(&absolute)?
        }
    }
}

/// The call and `cli_print_or_fail`, after the checks: `path` as resolved.
pub fn send(client: &mut Client, verb: Verb, ask: &Ask, path: &str) -> Exit {
    let mut closing = Closing::default();
    let passport = client.passport().is_some();
    let mut params = vec![format!("path={}", quote(path))];
    for (on, pair) in [
        (ask.no_wiki, "no_wiki=1"),
        (ask.no_agents_md, "no_agents_md=1"),
        (ask.fanout, "fanout=1"),
    ] {
        if on {
            params.push(pair.to_string());
        }
    }
    let refs: Vec<&str> = params.iter().map(String::as_str).collect();
    let exit = match client.call(Channel::Tcp, verb.endpoint(), Call::RequestPost(&refs)) {
        Ok(r) => {
            closing.answer(&r, passport);
            print_or_fail(&held(&r.body))
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            Exit::Error
        }
        Err(e) => Exit::from_curl(e.curl_code()),
    };
    closing.flush(client, exit);
    exit
}

/// What `cli_print_or_fail "$resp"` makes of `resp`, and its exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// `printf '%s\n' "$resp"`, exit 0.
    Print,
    /// `Error: <it>` on stderr; exit 3 when the body has a permission
    /// card's closing line, else 1.
    Error(String),
    /// The Python raised (its traceback to `/dev/null`): `set -e` ends the
    /// command with exit 1 and nothing printed.
    Raised,
}

/// `cli_print_or_fail`'s Python on `resp` (what `$(…)` held).
pub fn verdict(resp: &[u8]) -> Verdict {
    let text = String::from_utf8_lossy(resp);
    let Ok(d) = json::parse(&text) else {
        return Verdict::Print;
    };
    let Value::Object(pairs) = &d else {
        return Verdict::Raised;
    };
    let Some(e) = get(pairs, "error").filter(|e| py::truthy(e)) else {
        return Verdict::Print;
    };
    let said = match e {
        Value::Object(ep) => {
            let first = [get(ep, "hint"), get(ep, "code")]
                .into_iter()
                .flatten()
                .find(|v| py::truthy(v));
            first.map_or_else(|| "daemon refused".to_string(), py::str_of)
        }
        other => {
            let trust = ["folder_trust_pending", "folder_trust_denied"]
                .iter()
                .any(|w| *other == super::super::s(w));
            match get(pairs, "message").filter(|m| py::truthy(m)) {
                Some(m) if trust => format!("{}: {}", py::str_of(other), py::str_of(m)),
                _ => py::str_of(other),
            }
        }
    };
    // `err=$(…)`: NULs and trailing newlines go.
    let err: String = said.chars().filter(|c| *c != '\0').collect();
    let err = err.trim_end_matches('\n');
    if err.is_empty() {
        Verdict::Print
    } else {
        Verdict::Error(err.to_string())
    }
}

/// `cli_print_or_fail "$resp"`: prints and gives the exit.
pub fn print_or_fail(resp: &[u8]) -> Exit {
    match verdict(resp) {
        Verdict::Print => {
            let mut bytes = resp.to_vec();
            bytes.push(b'\n');
            match out::write_flushed(&mut std::io::stdout().lock(), &bytes) {
                Ok(()) => Exit::Ok,
                Err(e) => out::after_stdout_error(&e),
            }
        }
        Verdict::Error(err) => {
            out::stderr_line(&format!("Error: {err}"));
            if permission::note(resp).is_some() {
                Exit::Refused
            } else {
                Exit::Error
            }
        }
        Verdict::Raised => Exit::Error,
    }
}

/// What `$(…)` holds of a body: NULs and trailing newlines gone.
pub fn held(body: &[u8]) -> Vec<u8> {
    let mut b = without_nuls(body);
    while b.last() == Some(&b'\n') {
        b.pop();
    }
    b
}
