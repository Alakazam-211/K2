//! `k5 workspace`: the verbs a first run uses, as the shell CLI's
//! `workspace` arm (`cli/src/router.sh`) hands them to their functions
//! (`cli/src/agent-context.sh`, `cli/src/wiki.sh`).
//!
//! - [`list`]: `workspace list [--running] [--json]`, every workspace this
//!   server knows (`cmd_workspace_list`).
//! - [`register`]: `workspace create <path>` and `workspace open <path>`,
//!   how a folder becomes a workspace (`cmd_workspace_create`,
//!   `cmd_workspace_open`).
//! - [`agent`]: `workspace launch [<path>]`, `workspace profile [<path>]`
//!   and `workspace update --field <f> --value <v>`, the workspace's agent
//!   (`cmd_workspace_launch`, `cmd_workspace_profile`,
//!   `cmd_workspace_update`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//!    `workspace` with any of these verbs is an ungoverned tool, so no
//!    passport check comes after them.
//! 2. The verb's words, its requests and its output (each module says
//!    which). None of these verbs runs a program or writes a file of its
//!    own: the daemon does the work.
//! 3. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The route table sends each of these verbs here on its own verb row
//! ([`crate::route`]), so each can turn back to the shell alone; every
//! other verb, `workspace` alone and its help stay the shell's. The shell
//! CLI also answers these forms itself ([`run`] returns `None`): a command
//! line with `-h` or `--help` (the front sends those on,
//! [`crate::front`]); a word that is not UTF-8; a port that is set but is
//! no port curl takes; a `python3` the binary can't tell
//! ([`crate::families`], Without Python); and the forms each module names
//! (a path the binary can't resolve as bash's `cd` would, a flag with no
//! value after it where bash stops on `set -u`).
//!
//! Where this differs from the shell:
//!
//! - PRD 13.9 #4: in a session with `HOOK_SOCK` set and the owner token
//!   on disk, the shell's COMPAT-58 rule sends the owner token for these
//!   ungoverned verbs; this sends the passport.
//! - Without `python3` ([`crate::python::NATIVE_WITHOUT_PYTHON3`]) each
//!   verb answers as it does with one; the shell there stops at its first
//!   `urlencode` or Python reader with its own line.
//! - An answer with a NUL in it: bash 4.4 and later also print a warning
//!   of their own when `$(…)` drops it. An answer that is not UTF-8, and
//!   half of a surrogate pair, are as [`super::inbox`] says. The daemon
//!   sends neither.
//! - An answer cut short after its head (curl 18, or a reset mid-body):
//!   where the shell prints what came, this prints nothing.

use super::curl_port;
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude;
use crate::python;
use std::ffi::OsString;

pub mod agent;
pub mod list;
pub mod register;

/// The verbs the binary answers, as the router's `case` names them.
pub const VERBS: [&str; 6] = ["list", "launch", "profile", "update", "create", "open"];

/// Lines that stop a verb before it sends anything, and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub lines: Vec<String>,
    pub exit: Exit,
}

impl Stop {
    /// Prints the lines on stderr: the exit.
    pub fn print(&self) -> Exit {
        for l in &self.lines {
            out::stderr_line(l);
        }
        self.exit
    }
}

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Form {
    List(list::Ask),
    /// `profile`'s first word, if any.
    Profile(Option<String>),
    /// `launch`'s first word, if any.
    Launch(Option<String>),
    Update(Result<agent::Update, Stop>),
    Register(register::Verb, Result<register::Ask, Stop>),
}

/// The words after `workspace` (the verb first): the form, or `None` for
/// one the shell answers.
pub fn parse(words: &[&str]) -> Option<Form> {
    let (verb, rest) = words.split_first()?;
    let first = || rest.first().map(|w| (*w).to_string());
    Some(match *verb {
        "list" => Form::List(list::parse(rest)),
        "profile" => Form::Profile(first()),
        "launch" => Form::Launch(first()),
        "update" => Form::Update(agent::parse_update(rest)?),
        "create" => Form::Register(
            register::Verb::Create,
            register::parse(register::Verb::Create, rest),
        ),
        "open" => Form::Register(
            register::Verb::Open,
            register::parse(register::Verb::Open, rest),
        ),
        _ => return None,
    })
}

/// A form made ready to run: a register verb's path as the shell resolves
/// it.
enum Ready {
    Form(Form),
    Register(register::Verb, register::Ask, String),
}

/// `k5 workspace <verb> ...`: `args` is the whole command line after
/// `argv[0]`. `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    let form = parse(&words)?;
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
    let ready = match form {
        Form::Register(verb, Ok(ask)) => {
            let pwd = daemon::logical_cwd(std::env::var_os("PWD")).ok()?;
            let pwd = pwd.to_str()?.to_string();
            let cdpath = std::env::var_os("CDPATH").is_some();
            let path = register::path(verb, &ask.target, &pwd, cdpath)?;
            Ready::Register(verb, ask, path)
        }
        Form::Update(Ok(u)) => {
            // `basename "$PROJECT"` is only sure for an absolute path.
            agent::basename(&settings.project)?;
            Ready::Form(Form::Update(Ok(u)))
        }
        other => Ready::Form(other),
    };
    Some(run_with(args, ready, settings))
}

fn run_with(args: &[OsString], ready: Ready, mut settings: Settings) -> Exit {
    if let Some(gate) = prelude::gate(&settings) {
        return gate.stop();
    }
    match &ready {
        Ready::Form(Form::Update(Err(stop))) | Ready::Form(Form::Register(_, Err(stop))) => {
            return stop.print();
        }
        _ => {}
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
    // A lookup's `$(…)` subshell starts from the shell's state as it is
    // now, and what it heals stays there.
    let lookup = || settings.client(would_refuse).ok();
    let project = settings.project.clone();
    match ready {
        Ready::Form(Form::List(ask)) => list::send(&mut client, ask),
        Ready::Form(Form::Profile(target)) => agent::profile(&mut client, &project, target),
        Ready::Form(Form::Launch(target)) => match lookup() {
            Some(mut l) => agent::launch(&mut client, &mut l, &project, target),
            None => Exit::Error,
        },
        Ready::Form(Form::Update(Ok(u))) => match lookup() {
            Some(mut l) => agent::update(&mut client, &mut l, &project, &u),
            None => Exit::Error,
        },
        Ready::Register(verb, ask, path) => register::send(&mut client, verb, &ask, &path),
        // Printed above.
        Ready::Form(Form::Update(Err(_)) | Form::Register(..)) => Exit::Error,
    }
}

#[cfg(test)]
mod tests;
