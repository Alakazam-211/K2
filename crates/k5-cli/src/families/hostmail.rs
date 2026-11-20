//! `k5 hostmail`: K2 Mail's server administration, as the shell CLI's
//! `cmd_hostmail` (`cli/src/hostmail-*.sh`) answers it. Every verb runs
//! `_mail_py`, the same Python `mail` runs ([`super::mail::core`]).
//!
//! - [`approvals`]: `approvals` (list, show, save, approve, deny).
//! - [`forward`]: `forward` (set, add, rm, show, unset, pause, resume,
//!   list), whose Python branch `mail forward` shares
//!   ([`super::mail::forward`]).
//! - [`mailbox`]: `quota`, `catchall` and `alias`.
//! - [`notices`]: `ooo` and `footer`.
//! - [`words`]: how a `cmd_hostmail_*` function's loop reads its words.
//!
//! The route table sends a verb here only once it has a row of its own
//! ([`crate::route`]), and the bare line by its bare row ([`bare`]):
//! `hostmail` alone is the family's help page, exit 0, and
//! `hostmail --json` is `cmd_hostmail`'s unknown-command usage line, exit
//! 2, each after the gates. [`run`] returns `None` for anything else, and
//! the shell answers it.

use super::mail::core::{self, Plan};
use crate::exit::Exit;
use std::ffi::OsString;

pub mod admin;
pub mod approvals;
pub mod bans;
pub mod cert;
pub mod dns;
pub mod forward;
pub mod groups;
pub mod lists;
pub mod mailbox;
pub mod notices;
pub mod pyx;
pub mod server;
pub mod words;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["hostmail"];
/// The shell's but for [`VERBS`].
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// The verbs the binary answers, and the bare line ([`bare`]); every
/// other verb is the shell's. One verb row each, so each can turn back
/// alone.
pub const VERBS: &[crate::route::Verb] = &[
    crate::route::Verb::bare(crate::route::Owner::Native),
    native(&["forward"]),
    native(&["quota"]),
    native(&["catchall"]),
    native(&["alias"]),
    native(&["ooo"]),
    native(&["footer"]),
    native(&["enable"]),
    native(&["disable"]),
    native(&["uninstall"]),
    native(&["upgrade"]),
    native(&["rotate-admin"]),
    native(&["password"]),
    native(&["app-password"]),
    native(&["person"]),
    native(&["import"]),
    native(&["cert"]),
    native(&["outbound"]),
    native(&["backup"]),
    native(&["dkim"]),
    native(&["dmarc"]),
    native(&["ptr"]),
    native(&["profile"]),
    native(&["bans"]),
    native(&["allowlist"]),
    native(&["group"]),
    native(&["calendar"]),
    native(&["list"]),
    native(&["spam"]),
    native(&["queue"]),
    native(&["acl"]),
    native(&["autoconfig"]),
    native(&["audit-sieve"]),
    native(&["status"]),
    native(&["domain"]),
    native(&["create"]),
    native(&["delete"]),
    native(&["doctor"]),
    native(&["config"]),
    native(&["approvals"]),
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
        _ if crate::route::is_bare(args) => bare(args),
        _ => None,
    }
}

/// The verbs `cmd_hostmail`'s usage line names, in its order.
pub const USAGE_VERBS: &str = "status|enable|disable|uninstall|upgrade|rotate-admin|password|app-password|person|domain|create|delete|doctor|backup|config|approvals|import|quota|catchall|alias|forward|ooo|footer|cert|list|spam|queue|acl|autoconfig|dkim|dmarc|ptr|outbound|bans|allowlist|calendar|group|profile|audit-sieve";

/// `cmd_hostmail`'s `*)` hint for an unknown command.
pub fn unknown_command(sub: &str) -> String {
    format!(
        "unknown hostmail command '{sub}' \u{2014} usage: {} hostmail <{USAGE_VERBS}>",
        k2_client::brand::CLI_NAME
    )
}

/// The bare line (`hostmail`, `hostmail --json`), through the gates the
/// verbs take ([`core::run`]): `hostmail` alone is read there as an empty
/// verb word, which no verb is.
pub fn bare(args: &[OsString]) -> Option<Exit> {
    if args.len() == 1 {
        let padded = [args[0].clone(), OsString::new()];
        return core::run(&padded, bare_plan, super::mail::exec);
    }
    core::run(args, bare_plan, super::mail::exec)
}

/// `cmd_hostmail` with no verb: the help (`$# -eq 0`), or `--json` as an
/// unknown command.
pub fn bare_plan(verb: &str, words: &[&str]) -> Plan {
    match (verb, words) {
        ("", []) => Plan::Help("hostmail"),
        ("--json", []) => core::usage(unknown_command("--json")),
        _ => Plan::Shell,
    }
}

/// The words after `hostmail <verb>`, as the verb's `cmd_hostmail_*`
/// reads them.
pub fn parse(verb: &str, words: &[&str]) -> Plan {
    match verb {
        "forward" => forward::parse(words),
        v if mailbox::VERBS.contains(&v) => mailbox::parse(v, words),
        v if notices::VERBS.contains(&v) => notices::parse(v, words),
        v if server::VERBS.contains(&v) => server::parse(v, words),
        v if cert::VERBS.contains(&v) => cert::parse(v, words),
        v if dns::VERBS.contains(&v) => dns::parse(v, words),
        v if bans::VERBS.contains(&v) => bans::parse(v, words),
        v if groups::VERBS.contains(&v) => groups::parse(v, words),
        v if lists::VERBS.contains(&v) => lists::parse(v, words),
        v if admin::VERBS.contains(&v) => admin::parse(v, words),
        v if approvals::VERBS.contains(&v) => approvals::parse(v, words),
        _ => Plan::Shell,
    }
}

/// The `_mail_py` branches only `hostmail` runs, for `M_VERB` `verb`.
/// `None`: not one of them.
pub fn branch(py_: &mut core::Py, verb: &str) -> Option<core::R<()>> {
    mailbox::exec(py_, verb)
        .or_else(|| notices::exec(py_, verb))
        .or_else(|| server::exec(py_, verb))
        .or_else(|| cert::exec(py_, verb))
        .or_else(|| dns::exec(py_, verb))
        .or_else(|| bans::exec(py_, verb))
        .or_else(|| groups::exec(py_, verb))
        .or_else(|| lists::exec(py_, verb))
        .or_else(|| admin::exec(py_, verb))
        .or_else(|| approvals::exec(py_, verb))
}

/// `v="$(cat)"` for a secret flag (`--pass-stdin`, `--relay-pass-stdin`):
/// all of stdin, read where the loop meets the flag. Bytes that are not
/// UTF-8 read as U+FFFD, as `mail link add`'s do.
pub fn stdin_all() -> String {
    let s = crate::prompt::read_all_stdin();
    String::from_utf8(s.expose().to_vec()).unwrap_or_else(|_| "\u{fffd}".into())
}
