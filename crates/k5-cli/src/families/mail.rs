//! `k5 mail`: the agent surface of K2 Mail, as the shell CLI's `mail`
//! arm of `cli/src/router.sh` answers it.
//!
//! Every verb but `watch` goes through `_mail_py` (`cli/src/mail.sh`), the
//! shell's own urllib client over TCP. Those wait for the shared
//! urllib-shaped adapter (the plan's D4) and stay the shell's here
//! ([`run`] returns `None`).
//!
//! - [`watch`]: `k5 mail watch`, the mail notices, over `cli_send_json`
//!   (the transport's [`crate::transport::Client::send_json`]).

use crate::exit::Exit;
use std::ffi::OsString;

pub mod watch;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["mail"];
/// The shell's but for [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// Only `watch`; every other verb, and `mail` alone, is the shell's. One
/// verb row each, so each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[crate::route::Verb {
    words: &["watch"],
    owner: crate::route::Owner::Native,
}];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`): an
/// `id` tool, locked.
pub const TOOL: &str = "mail";

/// `k5 mail <verb> ...`: the verbs the binary answers. `None`: the shell's.
pub fn run(args: &[OsString]) -> Option<Exit> {
    match args.get(1).and_then(|w| w.to_str()) {
        Some("watch") => watch::run(args),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
