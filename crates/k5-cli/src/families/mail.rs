//! `k5 mail`: the agent surface of K2 Mail, as the shell CLI's `mail`
//! arm of `cli/src/router.sh` answers it.
//!
//! - [`watch`]: `k5 mail watch`, the mail notices, over `cli_send_json`
//!   (the transport's [`crate::transport::Client::send_json`]).
//! - [`core`]: `_mail_py` (`cli/src/mail.sh`), the shell's own urllib
//!   client, which every other verb runs: its `request`, `call` and
//!   `die`, and the printers its verbs share.
//! - [`reads`]: the reads (`inboxes`, `messages`, `read`, `wait`, …).
//! - [`py`]: the Python value rules the printers share.
//!
//! The route table sends a verb here only once it has a row of its own
//! ([`crate::route`]); [`run`] returns `None` for anything else, and the
//! shell answers it.

use crate::exit::Exit;
use std::ffi::OsString;

pub mod core;
pub mod py;
pub mod reads;
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
        Some(v) if reads::VERBS.contains(&v) => core::run(args, reads::parse, exec),
        _ => None,
    }
}

/// `_mail_py`'s verb chain: the branch for `M_VERB`.
fn exec(py_: &mut core::Py) -> core::R<()> {
    let verb = py_.env("M_VERB").to_string();
    match reads::exec(py_, &verb) {
        Some(r) => r,
        None => unreachable!("no _mail_py branch for {verb}"),
    }
}

#[cfg(test)]
mod tests;
