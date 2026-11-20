//! `k5 hostmail`: K2 Mail's server administration, as the shell CLI's
//! `cmd_hostmail` (`cli/src/hostmail-*.sh`) answers it. Every verb runs
//! `_mail_py`, the same Python `mail` runs ([`super::mail::core`]).
//!
//! - [`forward`]: `forward` (set, add, rm, show, unset, pause, resume,
//!   list), whose Python branch `mail forward` shares
//!   ([`super::mail::forward`]).
//! - [`mailbox`]: `quota`, `catchall` and `alias`.
//! - [`words`]: how a `cmd_hostmail_*` function's loop reads its words.
//!
//! The route table sends a verb here only once it has a row of its own
//! ([`crate::route`]); [`run`] returns `None` for anything else, and the
//! shell answers it.

use super::mail::core::{self, Plan};
use crate::exit::Exit;
use std::ffi::OsString;

pub mod forward;
pub mod mailbox;
pub mod words;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["hostmail"];
/// The shell's but for [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// The verbs the binary answers; every other verb, and `hostmail` alone,
/// is the shell's. One verb row each, so each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[
    native(&["forward"]),
    native(&["quota"]),
    native(&["catchall"]),
    native(&["alias"]),
];

/// A verb row the binary answers.
const fn native(words: &'static [&'static str]) -> crate::route::Verb {
    crate::route::Verb {
        words,
        owner: crate::route::Owner::Native,
    }
}

/// `k5 hostmail <verb> ...`: the verbs the binary answers. `None`: the
/// shell's.
pub fn run(args: &[OsString]) -> Option<Exit> {
    match args.get(1).and_then(|w| w.to_str()) {
        Some(v) if VERBS.iter().any(|r| r.words.contains(&v)) => {
            core::run(args, parse, super::mail::exec)
        }
        _ => None,
    }
}

/// The words after `hostmail <verb>`, as the verb's `cmd_hostmail_*`
/// reads them.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    match verb {
        "forward" => forward::parse(words),
        v if mailbox::VERBS.contains(&v) => mailbox::parse(v, words),
        _ => Plan::Shell,
    }
}

/// The `_mail_py` branches only `hostmail` runs, for `M_VERB` `verb`.
/// `None`: not one of them.
pub fn branch(py_: &mut core::Py, verb: &str) -> Option<core::R<()>> {
    mailbox::exec(py_, verb)
}
