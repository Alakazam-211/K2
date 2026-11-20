//! `k5 app <verb> …`, the App family, as the shell CLI's `cmd_app`
//! (`cli/src/app-wallet.sh`) answers it, one verb group at a time. The
//! route table sends a verb here when its group is native; every other
//! verb, and `app` alone, stays the shell's.
//!
//! Native so far:
//! - [`wallet`]: `app wallet settings|people|remove|personal-agent|personal-agents`.
//! - [`reads`]: `app list` (`--workspace`, `--box`), `app inspect` (not
//!   `--resources`) and `app deployment`.
//!
//! What runs, in the shell's order:
//!
//! 1. The settings and the gates, as for every verb that asks the daemon
//!    (`prelude.sh`). `app` is an `id` tool (`skin`): in a session, a
//!    stale passport and no passport at all are refused first
//!    ([`prelude::id_tool`]), and the session's passport is the
//!    credential.
//! 2. The group's own loop over the words: a usage mistake is its line on
//!    stderr and exit 2, before anything is sent.
//! 3. The group's requests ([`call`]): `_compute_call` and its refusal
//!    line, with the App refusal registry's `fix:` line.
//! 4. The printer: the answer as it came with `--json`, else the shell's
//!    Python, line for line (a Python 3.12 traceback where it raises).
//! 5. The EXIT trap's lines.
//!
//! The shell answers these itself ([`run`] returns `None`): a session
//! with `CODEX_*` set (the shared Codex lookup), a word that is not UTF-8, a verb no group here takes, a port curl won't take, and
//! a `python3` the binary can't tell ([`crate::families`], Without
//! Python). Help forms never reach here: the front sends them to their
//! page or to the shell.

use super::{curl_port, Shown};
use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude;
use crate::python;
use std::ffi::OsString;
use std::path::Path;

pub mod call;
pub mod py;
pub mod reads;
pub mod wallet;

use call::Ctx;

/// The catalog tool every `app` verb is (`_cli_tool_id_for_verb app`).
pub const TOOL: &str = "skin";

/// What the words ask for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ask {
    Wallet(wallet::Ask),
    List(reads::List),
    Deployment(reads::Deployment),
    Inspect(reads::Inspect),
}

/// Where a verb's loop stopped before anything was sent: its line on
/// stderr (none when empty), and the exit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub line: String,
    pub exit: Exit,
}

impl Stop {
    /// A usage line, exit 2.
    pub fn usage(line: impl Into<String>) -> Stop {
        Stop {
            line: line.into(),
            exit: Exit::Usage,
        }
    }

    /// Nothing said: `set -e` ended the command (a `shift 2` with one word
    /// left).
    pub fn silent(exit: Exit) -> Stop {
        Stop {
            line: String::new(),
            exit,
        }
    }
}

/// The words a group here takes (each verb row below names some of them).
pub const TAKES: &[&str] = &[
    "wallet",
    "list",
    "inspect",
    "deployment",
];

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["app"];
/// The shell's but for [`VERBS`]: `app` alone and every other verb are
/// the shell's.
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// One verb row per native group of `cmd_app`; each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[
    crate::route::Verb {
        words: &["wallet"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["list"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["inspect"],
        owner: crate::route::Owner::Native,
    },
    crate::route::Verb {
        words: &["deployment"],
        owner: crate::route::Owner::Native,
    },
];

/// The words after `app`, read by the group the verb names. `None`: the
/// shell's. `Err`: where the verb's loop stopped.
pub fn parse(words: &[&str]) -> Option<Result<Ask, Stop>> {
    let verb = *words.first()?;
    let rest = &words[1..];
    match verb {
        "wallet" => Some(wallet::parse(rest).map(Ask::Wallet).map_err(Stop::usage)),
        "list" => Some(reads::parse_list(rest)?.map(Ask::List)),
        "deployment" => Some(reads::parse_deployment(rest)?.map(Ask::Deployment)),
        "inspect" => Some(reads::parse_inspect(rest)?.map(Ask::Inspect)),
        _ => None,
    }
}

/// `k5 app …`. `None`: the shell's (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    let ask = parse(words.get(1..).unwrap_or_default())?;
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
        curl_port(settings.port_text.as_deref()?)?;
    }
    Some(run_with(args, &words, ask, hook_sock.as_deref(), settings))
}

fn run_with(
    args: &[OsString],
    words: &[&str],
    ask: Result<Ask, Stop>,
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
    let ask = match ask {
        Ok(a) => a,
        Err(stop) => {
            if !stop.line.is_empty() {
                out::stderr_line(&stop.line);
            }
            return stop.exit;
        }
    };
    settings.port = settings.port_text.as_deref().and_then(curl_port);
    let client = match settings.client(would_refuse) {
        Ok(c) => c,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Exit::Error;
        }
    };
    let mut ctx = Ctx::new(client, call::args_json(words));
    let shown = match &ask {
        Ask::Wallet(a) => wallet::run(&mut ctx, a),
        Ask::List(l) => reads::list(&mut ctx, l),
        Ask::Deployment(d) => reads::deployment(&mut ctx, d),
        Ask::Inspect(i) => reads::inspect(&mut ctx, i),
    };
    let exit = match shown {
        Ok(s) => s.print(),
        Err(exit) => exit,
    };
    let Ctx {
        mut client,
        closing,
        ..
    } = ctx;
    closing.flush(&mut client, exit);
    exit
}

/// A group's answer, printed: a [`Shown`], or the exit the requests ended
/// with (their lines printed).
pub type Answer = Result<Shown, Exit>;

#[cfg(test)]
mod tests;
