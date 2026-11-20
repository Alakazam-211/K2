//! `k5 workspace <verb>` for every verb a first run doesn't use, as the
//! shell CLI's `workspace` arm (`cli/src/router.sh`) hands them to their
//! functions, and `workspace` with no verb or one it doesn't know (its
//! usage line, exit 1).
//!
//! - [`switch`]: the per-workspace switches read from `/cli/settings` and
//!   written with `/cli/workspace/set`: `api-guest-policy`,
//!   `api-skip-permissions`, `default-model`, `force-model-on-resume`,
//!   `host-session-cell-cap` (and `api-host-session-cap`), `design-tab`
//!   (and `design-mode`) and `mobile-tab`.
//! - [`named`]: the verbs on one named workspace: `agent-name`,
//!   `set-agent-name`, `resume-chat-args`, `set-handle`, `api-key`,
//!   `swap-canonical`, and `cleanup`.
//! - [`trust`]: folder trust, `trust status|request|reset|forget` and the
//!   owner's decision.
//! - [`other`]: `triage`, `remove`, `resources` and `preview`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]). These verbs are open tools:
//!    no passport check comes after the gates, and the credential is the
//!    shell's (`_cli_apply_auth_for_verb`): the owner token outside a
//!    session, the owner token from disk in a session that has one, and
//!    the session's passport where there is no owner token. With both, the
//!    binary sends the passport (PRD 13.9 #4, as everywhere).
//! 2. The verb's words; a stop is its lines and exit, nothing sent.
//! 3. Most verbs first look the workspace up as the shell does:
//!    `resolved_path=$(cli_request /cli/workspace/resolve q=<target>
//!    2>/dev/null | python3 …)`, the answer's `path` or, when there is
//!    none, the target as typed ([`Sent::resolve`]). Under `set -e` a
//!    lookup that failed ends the command with curl's code, and one with
//!    no answer dies of SIGPIPE, each with no line (its stderr is thrown
//!    away).
//! 4. The verb's own request and its Python reader, on the left of a pipe
//!    or held in `$(…)` as the shell has it (each module says which).
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`): a
//! command line with `-h` or `--help` (the front sends those on); a word
//! that is not UTF-8; a port that is set but is no port curl takes; a
//! `python3` the binary can't tell ([`crate::families`]); a session with
//! no passport, for a verb that tries the cell socket (`cli_send`: the
//! shell tries it with an empty Bearer, or refuses the owner token); and
//! the forms each module names.
//!
//! `set-agent-identity` is the old name of `agent identity`
//! (`cmd_workspace_set_agent_identity`): the agent family answers it
//! ([`crate::families::agent::identity_alias`]), with the shell's note
//! ([`SET_AGENT_IDENTITY_NOTE`]) on stderr after the gates.
//!
//! Where this differs from the shell: as [`super`] says (an answer with a
//! NUL, one that is not UTF-8, half of a surrogate pair), and Python's
//! tracebacks are 3.12's. `json.loads`'s messages where `print("Error:
//! %s" % e)` shows them are Python's for the common cases
//! ([`decode_error`]).

use super::{held, Stop};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::{curl_port, end_in_pipe, piped_body, Piped, Raise, Shown};
use crate::out::json::{self, Value};
use crate::out::{self, py};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::encode::quote;
use crate::transport::{Auth, Call, Channel, Client, Error, Response};
use k2_client::cli_name;
use std::ffi::OsString;

pub use crate::families::remind::shared::{decode_error, py_get, py_in, py_index, str_plus};

pub mod named;
pub mod other;
pub mod switch;
pub mod trust;

/// `workspace` alone, or a verb the router's `case` doesn't name.
pub const USAGE: &str = concat!(
    "Usage: ",
    cli_name!(),
    " workspace <list|launch|profile|update|create|open|remove|resources|trust|triage|set-agent-name|set-agent-identity|set-handle|resume-chat-args|swap-canonical|api-key|api-guest-policy|api-skip-permissions|api-host-session-cap|host-session-cell-cap|default-model|force-model-on-resume|design-tab|mobile-tab|...> [args]"
);

/// `cmd_workspace_set_agent_identity`'s first line.
pub const SET_AGENT_IDENTITY_NOTE: &str = concat!(
    "note: '",
    cli_name!(),
    " workspace set-agent-identity' is now '",
    cli_name!(),
    " agent identity' (same flags)."
);

pub const RESOLVE: &str = "/cli/workspace/resolve";
pub const SETTINGS: &str = "/cli/settings";
pub const SET: &str = "/cli/workspace/set";

/// What the words ask for, ready to send (local files read, paths made
/// absolute), or the lines that stop it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    /// A form the shell answers (its help, and the ones each module
    /// names).
    Shell,
    Stop(Stop),
    /// A help page the verb prints itself (not the router's `-h`): on
    /// stdout, exit 0, after the gates.
    Page(&'static str),
    Switch(switch::Ask),
    Named(named::Ask),
    Trust(trust::Ask),
    Other(other::Ask),
}

impl Ask {
    /// The verb tries the cell socket first (`cli_send`).
    fn sends_on_socket(&self) -> bool {
        match self {
            Ask::Trust(t) => t.sends_on_socket(),
            Ask::Other(o) => o.sends_on_socket(),
            _ => false,
        }
    }
}

fn stop(lines: &[&str], exit: Exit) -> Ask {
    Ask::Stop(Stop {
        lines: lines.iter().map(|l| (*l).to_string()).collect(),
        exit,
    })
}

/// Where the words are read: `$PWD` as the shell has it, and whether
/// stdin and stdout are terminals.
pub struct Here {
    pub pwd: String,
    pub stdin_tty: bool,
    pub stdout_tty: bool,
}

/// The words after `workspace` (the verb first, none for `workspace`
/// alone). `None`: the shell's.
pub fn parse(words: &[&str], here: &Here) -> Option<Ask> {
    let Some((verb, rest)) = words.split_first() else {
        return Some(stop(&[USAGE], Exit::Error));
    };
    let ask = match *verb {
        "api-guest-policy"
        | "api-skip-permissions"
        | "default-model"
        | "force-model-on-resume"
        | "host-session-cell-cap"
        | "api-host-session-cap"
        | "design-tab"
        | "design-mode"
        | "mobile-tab" => switch::parse(verb, rest),
        "agent-name" | "set-agent-name" | "resume-chat-args" | "set-handle" | "api-key"
        | "swap-canonical" | "cleanup" => named::parse(verb, rest, here),
        "trust" => trust::parse(rest, here),
        "triage" | "remove" | "resources" | "preview" => other::parse(verb, rest, here),
        // `agent identity`'s old name: [`run`] hands it to the agent family.
        "set-agent-identity" => return None,
        _ => stop(&[USAGE], Exit::Error),
    };
    (ask != Ask::Shell).then_some(ask)
}

/// `k5 workspace <verb> ...` for the verbs here: `args` is the whole
/// command line after `argv[0]`. `None`: the shell CLI answers it.
pub fn run(args: &[OsString], words: &[&str]) -> Option<Exit> {
    if let Some((&"set-agent-identity", rest)) = words.split_first() {
        return crate::families::agent::identity_alias(args, rest, SET_AGENT_IDENTITY_NOTE);
    }
    let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
    let here = Here {
        pwd: pwd.to_str()?.to_string(),
        stdin_tty: is_tty(0),
        stdout_tty: is_tty(1),
    };
    let ask = parse(words, &here)?;
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
    if prelude::gate(&settings).is_none() {
        // A port curl won't take is the shell's.
        settings.port_text.as_deref().and_then(curl_port)?;
    }
    if ask.sends_on_socket() && settings.session && !matches!(settings.auth, Auth::Passport { .. })
    {
        return None;
    }
    Some(run_with(args, ask, settings))
}

fn run_with(args: &[OsString], ask: Ask, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    if let Ask::Stop(stop) = &ask {
        return stop.print();
    }
    if let Ask::Page(text) = &ask {
        return match out::stdout(text) {
            Ok(()) => Exit::Ok,
            Err(e) => out::after_stdout_error(&e),
        };
    }
    // `run` hands a port curl won't take to the shell.
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let would_refuse = passport::current(args).would_refuse();
    let mut client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let mut w = Sent {
        client: &mut client,
        closing: &mut closing,
        settings: &settings,
    };
    let exit = match ask {
        Ask::Switch(a) => switch::send(&mut w, &a),
        Ask::Named(a) => named::send(&mut w, &a),
        Ask::Trust(a) => trust::send(&mut w, &a),
        Ask::Other(a) => other::send(&mut w, &a),
        // Printed above, or the shell's.
        Ask::Stop(_) | Ask::Shell | Ask::Page(_) => Exit::Usage,
    };
    closing.flush(&mut client, exit);
    exit
}

/// What a verb's requests share.
pub struct Sent<'a> {
    pub client: &'a mut Client,
    pub closing: &'a mut Closing,
    pub settings: &'a Settings,
}

impl Sent<'_> {
    fn note(&mut self, r: &Response) {
        let passport = self.client.passport().is_some();
        self.closing.answer(r, passport);
    }

    /// `x=$(<call>)` under `set -e`: what `$(…)` holds (NULs and trailing
    /// newlines gone), or the exit the command ends with. A request with
    /// no answer, or a POST on the cell socket that may have been applied,
    /// is the transport's line (unless `quiet`: its stderr is thrown away)
    /// and exit 1, from the main shell's USR1 trap; any other failure is
    /// curl's code, with no line.
    pub fn held(&mut self, r: Result<Response, Error>, quiet: bool) -> Result<Vec<u8>, Exit> {
        match r {
            Ok(r) => {
                self.note(&r);
                Ok(held(&r.body))
            }
            Err(e) => Err(self.failed(&e, quiet)),
        }
    }

    /// The exit of a call that failed inside `$(…)`.
    pub fn failed(&mut self, e: &Error, quiet: bool) -> Exit {
        match e {
            Error::NoAnswer(_) | Error::MaybeApplied { .. } | Error::NoOwnerInSession { .. } => {
                if !quiet {
                    out::stderr_line(&e.to_string());
                }
                Exit::Error
            }
            e => Exit::from_curl(e.curl_code()),
        }
    }

    /// The exit of a call that failed on the left of a pipe inside
    /// `$(…)`: one with no answer is the transport's line (unless `quiet`)
    /// and the command dies of SIGPIPE ([`end_in_pipe`]); any other
    /// failure is curl's code, with no line.
    pub fn failed_in_pipe(&mut self, e: &Error, quiet: bool) -> Exit {
        match e {
            Error::NoAnswer(_) | Error::MaybeApplied { .. } => {
                if !quiet {
                    out::stderr_line(&e.to_string());
                }
                end_in_pipe()
            }
            e => self.failed(e, quiet),
        }
    }

    /// `<call> | python3 …` at the top: the left side of the pipe. A
    /// request with no answer is the transport's line, then the command
    /// dies of SIGPIPE ([`end_in_pipe`]).
    pub fn piped(&mut self, r: Result<Response, Error>) -> Piped {
        match r {
            Ok(r) => {
                self.note(&r);
                Piped {
                    body: piped_body(&r),
                    exit: Exit::Ok,
                }
            }
            Err(e @ (Error::NoAnswer(_) | Error::MaybeApplied { .. })) => {
                out::stderr_line(&e.to_string());
                end_in_pipe()
            }
            Err(e) => Piped {
                body: Vec::new(),
                exit: Exit::from_curl(e.curl_code()),
            },
        }
    }

    /// `cli_request <endpoint> <args>` over TCP.
    pub fn request(&mut self, endpoint: &str, args: &[&str]) -> Result<Response, Error> {
        self.client
            .call(Channel::Tcp, endpoint, Call::Request(args))
    }

    /// `cli_request_post <endpoint> <args>` over TCP.
    pub fn request_post(&mut self, endpoint: &str, args: &[&str]) -> Result<Response, Error> {
        self.client
            .call(Channel::Tcp, endpoint, Call::RequestPost(args))
    }

    /// `cli_post_json <endpoint> <body>` over TCP.
    pub fn post_json(&mut self, endpoint: &str, body: &str) -> Result<Response, Error> {
        self.client
            .call(Channel::Tcp, endpoint, Call::PostJson(body.as_bytes()))
    }

    /// The lookup: `resolved_path=$(cli_request /cli/workspace/resolve
    /// q=<target> 2>/dev/null | python3 …)`. `Err`: the exit when the
    /// lookup failed (`set -e`); `Ok`: the path, or `target` when the
    /// answer named none.
    pub fn resolve(&mut self, target: &str) -> Result<String, Exit> {
        let q = format!("q={}", quote(target));
        let r = self.request(RESOLVE, &[&q]);
        let body = match r {
            Ok(r) => {
                self.note(&r);
                piped_body(&r)
            }
            Err(e) => return Err(self.failed_in_pipe(&e, true)),
        };
        let path = resolved_of(&body);
        Ok(if path.is_empty() {
            target.to_string()
        } else {
            path
        })
    }

    /// `cli_request /cli/settings project=<path> | python3 …`.
    pub fn settings_of(&mut self, path: &str) -> Piped {
        let p = format!("project={}", quote(path));
        let r = self.request(SETTINGS, &[&p]);
        self.piped(r)
    }
}

/// The lookup's Python on what it read: `print(json.load(sys.stdin)
/// .get("path",""))`, nothing when it raised, as `$(…)` holds it.
pub fn resolved_of(body: &[u8]) -> String {
    let Ok(Value::Object(pairs)) = json::parse(&String::from_utf8_lossy(body)) else {
        return String::new();
    };
    let v = crate::families::get(&pairs, "path")
        .cloned()
        .unwrap_or_else(|| crate::families::s(""));
    if let Value::Str(t) = &v {
        if t.as_str().is_none() {
            // `print` can't write it out: it raised.
            return String::new();
        }
    }
    py::str_of(&v)
        .replace('\0', "")
        .trim_end_matches('\n')
        .to_string()
}

/// `isatty(fd)`.
#[cfg(unix)]
fn is_tty(fd: i32) -> bool {
    // SAFETY: isatty only reads the descriptor's state.
    unsafe { libc::isatty(fd) == 1 }
}

#[cfg(not(unix))]
fn is_tty(_fd: i32) -> bool {
    false
}

/// What a Python reader wrote, and where it raised.
#[derive(Debug, Default)]
pub struct Py {
    pub out: String,
    pub err: String,
}

impl Py {
    pub fn print(&mut self, line: &str) {
        self.out.push_str(line);
        self.out.push('\n');
    }

    pub fn eprint(&mut self, line: &str) {
        self.err.push_str(line);
        self.err.push('\n');
    }

    pub fn done(self, exit: Exit) -> Shown {
        Shown {
            stdout: self.out,
            stderr: self.err,
            exit,
        }
    }

    pub fn raised(self, r: &Raise) -> Shown {
        crate::families::raised(self.out, self.err, r)
    }
}

/// `raw[:n]`: the first `n` code points.
pub fn head(raw: &str, n: usize) -> String {
    raw.chars().take(n).collect()
}

/// `"%r" % raw[:n]`.
pub fn repr_head(raw: &str, n: usize) -> String {
    py::repr(&crate::families::s(&head(raw, n)))
}

/// `x == "<word>"` for a JSON value.
pub fn is_str(v: &Value, word: &str) -> bool {
    *v == crate::families::s(word)
}

/// `json.dumps(v)` with Python's defaults.
pub fn dumps(v: &Value) -> String {
    json::to_string(v, &json::Options::default()).unwrap_or_default()
}

/// A dict of string pairs, in order.
pub fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(k, v)| (json::Text::from(*k), v.clone()))
            .collect(),
    )
}

/// `print(...)` of the `Error: ...` line a set reader and a get reader
/// share: the daemon's `error`, a dict's `hint` or `code` or the dict.
pub fn error_text(err: &Value) -> String {
    if let Value::Object(_) = err {
        let hint = crate::families::get(as_pairs(err), "hint");
        let code = crate::families::get(as_pairs(err), "code");
        for v in [hint, code].into_iter().flatten() {
            if py::truthy(v) {
                return py::str_of(v);
            }
        }
    }
    py::str_of(err)
}

fn as_pairs(v: &Value) -> &[(json::Text, Value)] {
    match v {
        Value::Object(p) => p,
        _ => &[],
    }
}

/// Python's `str.strip()`.
pub fn py_strip(s: &str) -> &str {
    s.trim_matches(crate::families::py_space)
}

#[cfg(test)]
mod tests;
