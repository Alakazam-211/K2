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
//! 3. `on` and `off`: `resp=$(cli_post_json /cli/settings/update
//!    {"apiEnabled":true|false})` over TCP. `$(…)` drops the answer's NULs
//!    and trailing newlines ([`super::connections::held`]). A call that
//!    failed ends the command there with its code (`set -e`); one with no
//!    answer is the transport's line and exit 1. Then the shell's Python
//!    on `resp` ([`saved_reader`]): the daemon's error, an answer that is
//!    not JSON or a raise is what it printed, held by `saved=$(…)` and
//!    printed again with one newline, exit 1. Else the switch as the
//!    answer says it was saved (not as the verb asked):
//!    - on: the ON line;
//!    - off: `/boot-status` read as `status` reads it (step 4), then
//!      [`off_reader`] on `api.enabled`: `true` (an env flag forces the
//!      surface on) is [`STILL_ON`] on stderr, exit 1, no OFF line;
//!      `false` is the OFF line; anything else the OFF line and
//!      [`NOTE_OFF`].
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
//!
//! `on`/`off` hand both answers to Python in an environment variable. One
//! above what a variable may hold (128 KiB on Linux) stops bash there
//! ("Argument list too long"); the binary reads it, as in the cases of
//! the named difference `env-answer-large` (`scripts/cli-parity.sh`).
//! No row here: neither answer comes near that size.

use super::skin_token::py::{self, at, dict};
use super::connections::held;
use super::{curl_port, Raise, Shown};
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
/// The binary answers it ([`run`] may still hand a form to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
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
/// `off` when the surface stays on: on stderr, exit 1, no OFF line.
pub const STILL_ON: &str = "The public API is still ON: K2_API or K2_SANDBOX_API is set in the daemon's environment and forces it on. The setting is saved as off; to turn the API off, remove that variable from the daemon's service and restart it.";
/// `status` when the env flag forces the surface on.
pub const FORCED: &str = "  (forced on by the K2_API or legacy K2_SANDBOX_API env flag)";

/// The lines of the shell's Python that raise (`tests` pins them).
pub mod line {
    /// `on`/`off`'s `python3 -c '…'` on the settings answer.
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
        Ask::Switch(on) => switch(&mut client, &mut closing, on),
        Ask::Status => status(&mut client, &mut closing),
        Ask::Usage => Exit::Usage,
    };
    closing.flush(&mut client, exit);
    exit
}

/// `on`/`off`: the settings post, the Python on its answer, then for a
/// switch saved as off the surface as `/boot-status` gives it.
fn switch(client: &mut Client, closing: &mut Closing, on: bool) -> Exit {
    let body = format!("{{\"apiEnabled\":{on}}}");
    let passport = client.passport().is_some();
    let resp = match client.call(Channel::Tcp, UPDATE, Call::PostJson(body.as_bytes())) {
        Ok(r) => {
            closing.answer(&r, passport);
            held(&r.body)
        }
        Err(Error::NoAnswer(n)) => {
            out::stderr_line(&n.to_string());
            return Exit::Error;
        }
        Err(e) => return Exit::from_curl(e.curl_code()),
    };
    match saved_reader(&String::from_utf8_lossy(&resp)) {
        Saved::On => Shown::out(format!("{}\n", setting_line(true))).print(),
        Saved::Failed(shown) => shown.print(),
        Saved::Off => {
            let boot = match client.get_boot_status(BOOT) {
                Ok(r) => held(&r.body),
                Err(_) => Vec::new(),
            };
            off_reader(&String::from_utf8_lossy(&boot)).print()
        }
    }
}

/// `Public /v1 API setting: ON|OFF (…)`.
pub fn setting_line(on: bool) -> String {
    format!(
        "Public /v1 API setting: {} (live now \u{2014} no restart needed)",
        if on { "ON" } else { "OFF" }
    )
}

/// What `saved=$(python3 …)` made of the settings answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    On,
    Off,
    /// The Python failed: what the shell prints then (`printf '%s\n'
    /// "$saved"` on stdout, Python's traceback on stderr), exit 1.
    Failed(Shown),
}

/// The Python on the settings answer (what `resp=$(…)` held).
pub fn saved_reader(raw: &str) -> Saved {
    let failed = |stdout: &str, stderr: String| {
        // `$(…)` drops the NULs and the trailing newlines; printf adds one.
        let held: String = stdout.chars().filter(|c| *c != '\0').collect();
        Saved::Failed(Shown {
            stdout: format!("{}\n", held.trim_end_matches('\n')),
            stderr,
            exit: Exit::Error,
        })
    };
    let Ok(d) = crate::out::json::parse(raw) else {
        let head = Value::Str(Text::from(py::head(raw, 200)));
        return failed(&format!("{UNPARSEABLE}{}\n", pyout::repr(&head)), String::new());
    };
    match saved_on(&d) {
        Ok(Ok(true)) => Saved::On,
        Ok(Ok(false)) => Saved::Off,
        Ok(Err(line)) => failed(&line, String::new()),
        Err(r) => failed("", r.text()),
    }
}

/// The switch as saved, or the daemon's `Error: …` line.
fn saved_on(d: &Value) -> Result<Result<bool, String>, Raise> {
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
        return Ok(Err(format!("Error: {}\n", pyout::str_of(&err))));
    }
    let d = dict(d, line::switch::GET)?;
    Ok(Ok(at(d, "apiEnabled").is_some_and(pyout::truthy)))
}

/// The surface as the second Python reads `/boot-status` (what `boot=$(…)`
/// held): `api.enabled` when it is `true` or `false`, else unknown.
pub fn surface(boot: &str) -> Option<bool> {
    let d = parse_or_empty(boot);
    match at(&d, "api").and_then(|api| at(api, "enabled")) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// After a switch saved as off: the lines for the surface `/boot-status`
/// gives.
pub fn off_reader(boot: &str) -> Shown {
    match surface(boot) {
        Some(true) => Shown::err(format!("{STILL_ON}\n"), Exit::Error),
        Some(false) => Shown::out(format!("{}\n", setting_line(false))),
        None => Shown::out(format!("{}\n{NOTE_OFF}\n", setting_line(false))),
    }
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
        stdout.push_str(&format!("Public /v1 API surface: {}\n", fmt(eff.as_ref())));
        stdout.push_str(&format!(
            "  Settings switch:      {}\n",
            fmt(setting.as_ref())
        ));
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
            stderr: r.text().replace("File \"<string>\"", "File \"<stdin>\""),
            exit: Exit::Error,
        },
    }
}

#[cfg(test)]
mod tests;
