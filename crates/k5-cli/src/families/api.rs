//! `k5 api on|off|status`, the public /v1 API switch, as the shell CLI's
//! `cmd_api` answers it (`cli/src/talk-and-workspace.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the two gates ([`crate::prelude`]). `api` is no
//!    tool in the shell's policy (an open verb): no `id` checks. In a
//!    session with a passport the shell sends the owner token from disk
//!    when there is one (COMPAT-58); the binary sends the passport
//!    (`scripts/cli-parity.sh`'s named difference
//!    `owner-token-with-passport`, PRD 13.9 #4).
//! 2. The verb, `${1:-status}`; an unknown one is three usage lines on
//!    stderr, exit 2.
//! 3. `on` and `off`: `cli_post_json /cli/settings/update
//!    {"apiEnabled":true|false}` over TCP, on the left of a pipe into the
//!    shell's Python ([`switch_reader`]), whose lines all go to stdout. No
//!    answer is the transport's line, then death by SIGPIPE
//!    ([`super::end_in_pipe`]).
//! 4. `status`: `_k2_curl` of `/boot-status` with the token as Bearer, at
//!    most 10 s, any failure read as nothing; then `cli_request
//!    /cli/settings/get` (no answer: the line and exit 1; any other failure
//!    read as nothing); then the Python on both ([`status_reader`]).
//! 5. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell answers itself: a line with `-h` or `--help` (the front sends
//! those on), a word that is not UTF-8, a port curl won't take, and a
//! `python3` the binary can't tell. Where the shell's Python raises, this
//! prints a Python 3.12 traceback.

use super::skin_token::py::{self, at, dict};
use super::{curl_port, end_in_pipe, piped_body, pipefail, raised, Piped, Raise, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out::json::{Text, Value};
use crate::out::{self, py as pyout};
use crate::passport;
use crate::prelude::{self, Closing};
use crate::python;
use crate::transport::{Call, Channel, Client, Error};
use k2_client::cli_name;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["api"];
/// The shell answers it until the flip.
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The routes.
pub const UPDATE: &str = "/cli/settings/update";
pub const SETTINGS: &str = "/cli/settings/get";
pub const BOOT: &str = "/boot-status";

/// An unknown verb: exit 2.
pub const USAGE: [&str; 3] = [
    concat!("Usage: ", cli_name!(), " api <on|off|status>"),
    "  on/off  \u{2014} flip the public /v1 API surface (owner/admin; live immediately, no restart)",
    "  status  \u{2014} effective gate (env OR setting) plus the persisted Settings switch",
];
/// `on`/`off`'s answer that is not JSON, before its `%r`.
pub const UNPARSEABLE: &str = "Error: daemon returned an unparseable response: ";
/// After `off`.
pub const NOTE_OFF: &str = concat!(
    "Note: where the K2_API (or legacy K2_SANDBOX_API) env flag is set, it still forces the surface on; check with: ",
    cli_name!(),
    " api status"
);
/// `status` when the env flag forces the surface on.
pub const FORCED: &str = "  (forced on by the K2_API or legacy K2_SANDBOX_API env flag)";

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// `on`/`off`'s `python3 -c "…"`.
    pub mod switch {
        pub const IN: u32 = 9;
        pub const INDEX: u32 = 10;
        pub const GET: u32 = 12;
    }
    /// `status`'s heredoc.
    pub mod status {
        pub const BOOT: u32 = 9;
        pub const SETTINGS: u32 = 10;
    }
}

/// What the words ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ask {
    Switch(bool),
    Status,
    Usage,
}

/// The words after `api`.
pub fn parse(words: &[&str]) -> Ask {
    match words.first().copied().unwrap_or("") {
        "on" => Ask::Switch(true),
        "off" => Ask::Switch(false),
        "status" | "" => Ask::Status,
        _ => Ask::Usage,
    }
}

/// `k5 api ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let ask = parse(&words);
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
    Some(run_with(args, ask, settings))
}

fn run_with(args: &[OsString], ask: Ask, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    if ask == Ask::Usage {
        for l in USAGE {
            out::stderr_line(l);
        }
        return Exit::Usage;
    }
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let mut client = match settings.client(passport::current(args).would_refuse()) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut closing = Closing::default();
    let exit = match ask {
        Ask::Switch(on) => {
            let left = post_piped(&mut client, &mut closing, on);
            let shown = switch_reader(&String::from_utf8_lossy(&left.body));
            pipefail(left.exit, shown.print())
        }
        Ask::Status => status(&mut client, &mut closing),
        Ask::Usage => Exit::Usage,
    };
    closing.flush(&mut client, exit);
    exit
}

/// `cli_post_json /cli/settings/update '{"apiEnabled":<on>}'` on the left
/// of a pipe.
fn post_piped(client: &mut Client, closing: &mut Closing, on: bool) -> Piped {
    let body = format!("{{\"apiEnabled\":{on}}}");
    let passport = client.passport().is_some();
    match client.call(Channel::Tcp, UPDATE, Call::PostJson(body.as_bytes())) {
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

/// `on`/`off`'s Python on what the call printed.
pub fn switch_reader(raw: &str) -> Shown {
    let Ok(d) = crate::out::json::parse(raw) else {
        let head = Value::Str(Text::from(py::head(raw, 200)));
        return Shown {
            stdout: format!("{UNPARSEABLE}{}\n", pyout::repr(&head)),
            stderr: String::new(),
            exit: Exit::Error,
        };
    };
    let mut stdout = String::new();
    match switch_lines(&d, &mut stdout) {
        Ok(exit) => Shown {
            stdout,
            stderr: String::new(),
            exit,
        },
        Err(r) => raised(stdout, String::new(), &r),
    }
}

fn switch_lines(d: &Value, o: &mut String) -> Result<Exit, Raise> {
    if py::py_in("error", d, line::switch::IN)? {
        let err = match d {
            Value::Object(_) => at(d, "error").cloned().unwrap_or(Value::Null),
            Value::Array(_) => {
                return Err(Raise::at(
                    line::switch::INDEX,
                    "TypeError: list indices must be integers or slices, not str",
                ))
            }
            _ => {
                return Err(Raise::at(
                    line::switch::INDEX,
                    "TypeError: string indices must be integers, not 'str'",
                ))
            }
        };
        o.push_str(&format!("Error: {}\n", pyout::str_of(&err)));
        return Ok(Exit::Error);
    }
    let d = dict(d, line::switch::GET)?;
    let on = at(d, "apiEnabled").is_some_and(pyout::truthy);
    o.push_str(&format!(
        "Public /v1 API setting: {} (live now \u{2014} no restart needed)\n",
        if on { "ON" } else { "OFF" }
    ));
    if !on {
        o.push_str(NOTE_OFF);
        o.push('\n');
    }
    Ok(Exit::Ok)
}

/// `status`: the two reads, then the Python.
fn status(client: &mut Client, closing: &mut Closing) -> Exit {
    let boot = match client.get_boot_status(BOOT) {
        Ok(r) => super::connections::held(&r.body),
        Err(_) => Vec::new(),
    };
    let passport = client.passport().is_some();
    let settings = match client.call(Channel::Tcp, SETTINGS, Call::Request(&[])) {
        Ok(r) => {
            closing.answer(&r, passport);
            super::connections::held(&r.body)
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Exit::Error;
        }
        Err(_) => Vec::new(),
    };
    status_reader(
        &String::from_utf8_lossy(&boot),
        &String::from_utf8_lossy(&settings),
    )
    .print()
}

/// `parse(name)`: the value, or `{}` when it is empty or not JSON.
fn parse_or_empty(text: &str) -> Value {
    let text = if text.is_empty() { "{}" } else { text };
    crate::out::json::parse(text).unwrap_or(Value::Object(Vec::new()))
}

/// `fmt(v)`.
fn fmt(v: Option<&Value>) -> &'static str {
    match v {
        None | Some(Value::Null) => "?",
        Some(v) if pyout::truthy(v) => "on",
        Some(_) => "off",
    }
}

/// `status`'s heredoc on the two answers (each as `$(…)` held it).
pub fn status_reader(boot: &str, settings: &str) -> Shown {
    let boot = parse_or_empty(boot);
    let settings = parse_or_empty(settings);
    let mut stdout = String::new();
    let r = (|| -> Result<(), Raise> {
        let boot = dict(&boot, line::status::BOOT)?;
        let api = at(boot, "api")
            .cloned()
            .unwrap_or(Value::Object(Vec::new()));
        let api = dict(&api, line::status::BOOT)?;
        let eff = at(api, "enabled").cloned();
        let settings = dict(&settings, line::status::SETTINGS)?;
        let setting = at(settings, "apiEnabled").cloned();
        stdout.push_str(&format!(
            "Public /v1 API surface: {}\n",
            fmt(eff.as_ref())
        ));
        stdout.push_str(&format!("  Settings switch:      {}\n", fmt(setting.as_ref())));
        if eff.as_ref().is_some_and(pyout::truthy) && setting == Some(Value::Bool(false)) {
            stdout.push_str(FORCED);
            stdout.push('\n');
        }
        Ok(())
    })();
    match r {
        Ok(()) => Shown::out(stdout),
        // A heredoc's frames name `<stdin>`.
        Err(r) => Shown {
            stdout,
            stderr: r
                .text()
                .replace("File \"<string>\"", "File \"<stdin>\""),
            exit: Exit::Error,
        },
    }
}

#[cfg(test)]
mod tests;
