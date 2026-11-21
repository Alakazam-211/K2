//! `k5 thread`: the Thread beside an agent's terminal (the overlay, not
//! the PTY), as the shell CLI's `cmd_thread` answers it
//! (`cli/src/browser.sh`, `cli/src/thread.sh`, `_thread_md_nudge` in
//! `cli/src/prelude.sh`).
//!
//! What runs, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `thread` is an `id` tool in the shell's policy
//!    (`_cli_tool_id_for_verb`: `msg|talk|thread`), so with `HOOK_SOCK`
//!    set: the stale-passport or require-passport lines, exit 3
//!    ([`crate::prelude::id_tool`]).
//! 3. The first word that is not a flag or a flag's value ([`first_word`])
//!    picks the verb:
//!    - [`post`]: a write (`thread <addr> "text"`, `--file`) or a read
//!      (`--limit`, `--all`, `--before-seq`, `--seq`); R3.5;
//!    - [`cards`]: `ask`, `secret`, `withdraw` (`void`), `dismiss` and
//!      `delete`; R3.6;
//!    - [`marks`]: `read-all` and `unread`;
//!    - `post`: the line that there is no such verb, exit 1.
//! 4. The EXIT trap's lines ([`crate::prelude::Closing`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`):
//!
//! - a command line with `-h` or `--help` (the front sends those on,
//!   [`crate::front`]);
//! - `export --since-days` with a digit outside ASCII ([`transfer`]);
//! - `HOOK_SOCK` set with a `CODEX_` variable exported
//!   ([`crate::prelude::codex_env`]), a word that is not UTF-8, a port
//!   curl won't take, and a `python3` the binary can't tell;
//! - a `--file` the binary can't resolve as Python would (a digit
//!   outside ASCII, a folder that is not UTF-8), named in [`post`].

use crate::exit::Exit;
use crate::python;
use std::ffi::OsString;

pub mod cards;
pub mod marks;
pub mod post;
pub mod py;
pub mod transfer;

#[cfg(test)]
mod tests;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["thread"];
/// The binary, by [`run`] (which may still hand a line to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "thread";

/// The flags whose next word `cmd_thread` skips when it looks for the
/// verb.
pub const VALUE_FLAGS: &[&str] = &[
    "--from",
    "--options",
    "--option",
    "--name",
    "--prompt",
    "--dest",
    "--timeout",
    "--style",
    "--limit",
    "--before-seq",
    "--seq",
    "--file",
    "--scope",
    "--for",
    "--delivery",
    "--kind",
    "--ttl",
    "--inject-dst",
    "--inject-header",
    "--inject-format",
    "--why",
    "--reason",
    "--idempotency-key",
];

/// `cmd_thread`'s `first_pos`: the first word that is not a `--` flag or
/// a value flag's value (empty for none).
pub fn first_word<'a>(words: &[&'a str]) -> &'a str {
    let mut skip = false;
    for w in words {
        if skip {
            skip = false;
            continue;
        }
        if VALUE_FLAGS.contains(w) {
            skip = true;
            continue;
        }
        if w.starts_with("--") {
            continue;
        }
        return w;
    }
    ""
}

/// `post`: no such verb.
pub fn no_post() -> String {
    format!(
        "Error: no 'post' subcommand — use  {} thread <addr> \"text\"",
        k2_client::brand::CLI_NAME
    )
}

/// `k5 thread ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && crate::prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "--help" | "-h")) {
        return None;
    }
    if !python::family_native(python::here()) {
        return None;
    }
    let verb = first_word(&words);
    if matches!(verb, "export" | "import" | "imports") {
        let parsed = transfer::parse(&words)?;
        let (mut client, _) = match super::msg::client_for(args, hook_sock.as_deref(), TOOL)? {
            Ok(c) => c,
            Err(exit) => return Some(exit),
        };
        return Some(match parsed {
            transfer::Parsed::Stop(line, exit) => {
                if !line.is_empty() {
                    crate::out::stderr_line(&line);
                }
                exit
            }
            transfer::Parsed::Ask(a) => transfer::run(&mut client, a),
        });
    }
    let parsed = match verb {
        "ask" | "secret" | "void" | "withdraw" | "dismiss" | "delete" => {
            Verb::Card(cards::parse(verb, &words)?)
        }
        "read-all" | "unread" => Verb::Mark(marks::parse(verb, &words)),
        "post" => Verb::Post,
        _ => Verb::Thread(post::parse(&words)?),
    };
    let (mut client, _) = match super::msg::client_for(args, hook_sock.as_deref(), TOOL)? {
        Ok(c) => c,
        Err(exit) => return Some(exit),
    };
    let mut closing = crate::prelude::Closing::default();
    let exit = match parsed {
        Verb::Post => {
            crate::out::stderr_line(&no_post());
            Exit::Error
        }
        Verb::Thread(p) => post::run(&mut client, &mut closing, p),
        Verb::Card(c) => cards::run(&mut client, &mut closing, c),
        Verb::Mark(m) => marks::run(&mut client, &mut closing, m),
    };
    closing.flush(&mut client, exit);
    Some(exit)
}

/// What the words come to, before anything is sent.
enum Verb {
    Post,
    Thread(post::Parsed),
    Card(cards::Parsed),
    Mark(marks::Parsed),
}
