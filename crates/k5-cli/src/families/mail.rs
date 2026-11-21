//! `k5 mail`: the agent surface of K2 Mail, as the shell CLI's `mail`
//! arm of `cli/src/router.sh` answers it.
//!
//! - [`watch`]: `k5 mail watch`, the mail notices, over `cli_send_json`
//!   (the transport's [`crate::transport::Client::send_json`]).
//! - [`core`]: `_mail_py` (`cli/src/mail.sh`), the shell's own urllib
//!   client, which every other verb runs: its `request`, `call` and
//!   `die`, and the printers its verbs share.
//! - [`reads`]: the reads (`inboxes`, `messages`, `read`, `wait`, …).
//! - [`writes`]: the writes, each sent once (`send`, `reply`, `move`, …).
//! - [`owner`]: the owner's verbs (`link`, `access`, `oauth-config`,
//!   `sending`).
//! - [`forward`]: `forward`, a person's own mailbox forwards (the
//!   Python branch is `hostmail forward`'s too).
//! - [`py`]: the Python value rules the printers share.
//! - [`reader`]: `mail reader`, the mail reader for people.
//! - The retired owner verbs ([`RETIRED`]): each is the router's
//!   moved-verb usage line, exit 2, after the gates.
//! - The bare line ([`bare`]): `mail` alone is the family's help page,
//!   exit 0; `mail --json` the unknown-command line, exit 2; each after
//!   the gates.
//!
//! The route table sends a verb here only once it has a row of its own
//! ([`crate::route`]); [`run`] returns `None` for anything else, and the
//! shell answers it.

use crate::exit::Exit;
use std::ffi::OsString;

pub mod core;
pub mod forward;
pub mod owner;
pub mod py;
pub mod reader;
pub mod reads;
pub mod watch;
pub mod writes;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["mail"];
/// The shell's but for [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// `watch`, the reads, the writes, the owner's verbs, `forward`, `reader`
/// and the retired verbs, and the bare line ([`bare`]); every other verb
/// is the shell's. One verb row each, so each can turn back alone.
pub const VERBS: &[crate::route::Verb] = &[
    crate::route::Verb::bare(crate::route::Owner::Native),
    native(&["watch"]),
    native(&["inboxes"]),
    native(&["addresses"]),
    native(&["list"]),
    native(&["domains"]),
    native(&["messages"]),
    native(&["read"]),
    native(&["attachments"]),
    native(&["wait"]),
    native(&["drafts"]),
    native(&["outbox"]),
    native(&["capabilities"]),
    native(&["folder"]),
    native(&["send"]),
    native(&["reply"]),
    native(&["forward-message"]),
    native(&["draft"]),
    native(&["move"]),
    native(&["flag"]),
    native(&["archive"]),
    native(&["delete"]),
    native(&["link"]),
    native(&["external"]),
    native(&["access"]),
    native(&["oauth-config"]),
    native(&["sending"]),
    native(&["forward"]),
    native(&["reader"]),
    native(&["create"]),
    native(&["status"]),
    native(&["domain"]),
    native(&["doctor"]),
    native(&["config"]),
    native(&["approvals"]),
    native(&["catchall"]),
    native(&["alias"]),
    native(&["ooo"]),
    native(&["footer"]),
    native(&["spam"]),
    native(&["queue"]),
    native(&["acl"]),
    native(&["autoconfig"]),
    native(&["app-password"]),
    native(&["person"]),
    native(&["dkim"]),
    native(&["dmarc"]),
    native(&["ptr"]),
    native(&["bans"]),
    native(&["allowlist"]),
    native(&["audit-sieve"]),
];

/// The owner verbs that moved to `hostmail`, in the router arm's order:
/// `_mail_err "usage" "'k2 mail <verb>' moved to 'k2 hostmail <verb>'
/// (server admin)"`, exit 2, whatever follows the verb.
pub const RETIRED: &[&str] = &[
    "create",
    "status",
    "domain",
    "doctor",
    "config",
    "approvals",
    "catchall",
    "alias",
    "ooo",
    "footer",
    "spam",
    "queue",
    "acl",
    "autoconfig",
    "app-password",
    "person",
    "dkim",
    "dmarc",
    "ptr",
    "bans",
    "allowlist",
    "audit-sieve",
];

/// A retired verb's line: the moved-verb hint.
pub fn retired(verb: &str, _words: &[&str]) -> core::Plan {
    let cli = k2_client::brand::CLI_NAME;
    core::usage(format!(
        "'{cli} mail {verb}' moved to '{cli} hostmail {verb}' (server admin)"
    ))
}

/// A verb row the binary answers.
const fn native(words: &'static [&'static str]) -> crate::route::Verb {
    crate::route::Verb {
        words,
        owner: crate::route::Owner::Native,
    }
}

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`): an
/// `id` tool, locked.
pub const TOOL: &str = "mail";

/// `k5 mail <verb> ...`: the verbs the binary answers. `None`: the shell's.
pub fn run(args: &[OsString]) -> Option<Exit> {
    match args.get(1).and_then(|w| w.to_str()) {
        Some("watch") => watch::run(args),
        Some(v) if reads::VERBS.contains(&v) => core::run(args, reads::parse, exec),
        Some(v) if writes::VERBS.contains(&v) => core::run(args, writes::parse, exec),
        Some(v) if owner::VERBS.contains(&v) => core::run(args, owner::parse, exec),
        Some(v) if forward::VERBS.contains(&v) => core::run(args, forward::parse, exec),
        Some("reader") => core::run(args, reader::parse, exec),
        Some(v) if RETIRED.contains(&v) => core::run(args, retired, exec),
        _ if crate::route::is_bare(args) => bare(args),
        _ => None,
    }
}

/// The bare line (`mail`, `mail --json`), through the gates the verbs
/// take ([`core::run`]): `mail` alone is read there as an empty verb
/// word.
pub fn bare(args: &[OsString]) -> Option<Exit> {
    if args.len() == 1 {
        let padded = [args[0].clone(), OsString::new()];
        return core::run(&padded, bare_plan, exec);
    }
    core::run(args, bare_plan, exec)
}

/// The router's `mail` arm with no verb: `''` is `cmd_help_mail`, exit
/// 0; `--json` is no verb (`*)`: the unknown-command line, exit 2).
pub fn bare_plan(verb: &str, words: &[&str]) -> core::Plan {
    match (verb, words) {
        ("", []) => core::Plan::Help("mail"),
        ("--json", []) => core::usage(format!(
            "unknown command '--json' \u{2014} see {} mail --help",
            k2_client::brand::CLI_NAME
        )),
        _ => core::Plan::Shell,
    }
}

/// `_mail_py`'s verb chain: the branch for `M_VERB`. `hostmail` runs the
/// same Python.
pub fn exec(py_: &mut core::Py) -> core::R<()> {
    let verb = py_.env("M_VERB").to_string();
    reads::exec(py_, &verb)
        .or_else(|| writes::exec(py_, &verb))
        .or_else(|| owner::exec(py_, &verb))
        .or_else(|| forward::exec(py_, &verb))
        .or_else(|| reader::exec(py_, &verb))
        .or_else(|| super::hostmail::branch(py_, &verb))
        .unwrap_or_else(|| unreachable!("no _mail_py branch for {verb}"))
}

#[cfg(test)]
mod tests;
