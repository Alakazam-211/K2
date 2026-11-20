//! `k5 msg`: a message to another workspace's agent, as the shell CLI's
//! router arm sends it (`cli/src/router.sh`, `msg)`):
//!
//! - [`who`]: `k5 msg who <m_…> [--json]`, who sent a delivery
//!   (`cmd_msg_who`, `cli/src/agent-template.sh`).
//! - [`verify`]: `k5 msg verify <code> [--text … | --text-file …] [--json]`,
//!   who sent it and whether the text is the one delivered
//!   (`cmd_msg_verify`, `cli/src/msg-verify.sh`).
//! - [`send`]: everything else, `cmd_msg_v2` (`cli/src/msg-inbox-staging.sh`)
//!   and its live form `cmd_msg` (`cli/src/msg-whoami-done.sh`), sent once
//!   by its idempotency key ([`send_once`]), and Postal mail ([`postal`]).
//!
//! What runs before any of them, in the shell's order:
//!
//! 1. The settings ([`crate::daemon`]), a re-minted passport taken at once,
//!    and the two gates ([`crate::prelude`]).
//! 2. `msg` is an `id` tool in the shell's policy (`_cli_tool_id_for_verb`:
//!    `msg|talk|thread`), so with `HOOK_SOCK` set: the stale-passport lines
//!    or the require-passport lines, exit 3 ([`crate::prelude::id_tool`]).
//!
//! The shell CLI answers these forms itself ([`run`] returns `None`), so
//! there is one copy of each:
//!
//! - a command line with `-h` or `--help` (the front sends those on,
//!   [`crate::front`]);
//! - `HOOK_SOCK` set with a `CODEX_` variable exported, where the shell
//!   looks for a shared Codex server in the process's ancestry first
//!   ([`crate::prelude::codex_env`]);
//! - a word that is not UTF-8;
//! - the forms [`send`] names (`--signal`, `--inbox-silent`
//!   and `--inbox-wake`, `--secret`).

use super::curl_port;
use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude;
use crate::transport::Client;
use k2_client::env_names;
use std::ffi::OsString;
use std::path::Path;

pub mod postal;
pub mod py;
pub mod remote;
pub mod send;
pub mod send_once;
pub mod verify;
pub mod who;

#[cfg(test)]
mod tests;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["msg"];
/// The binary, by [`run`] (which may still hand a line to the shell).
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy (`_cli_tool_id_for_verb`).
pub const TOOL: &str = "msg";

/// `k5 msg ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it (see the module's notes).
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = super::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(name, _)| name)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    match words.first().copied() {
        Some("who") => who::run(args, &words[1..], hook_sock.as_deref()),
        Some("verify") => verify::run(args, &words[1..], hook_sock.as_deref()),
        _ => send::run(args, &words, hook_sock.as_deref()),
    }
}

/// Steps 1 and 2 of the module's notes: the client the verb sends with,
/// or the exit when a gate stopped it (its lines printed). `None`: a port
/// curl won't take, which the shell meets in its own way on each request
/// (`_k2_curl`'s 3, or `_cli_curl`'s 76 when `TMPDIR` can't be used), so
/// the shell answers it.
pub fn client(args: &[OsString], hook_sock: Option<&Path>) -> Option<Result<Client, Exit>> {
    let mut settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Err(Exit::Error));
        }
    };
    if let Some(gate) = prelude::gate(&settings) {
        return Some(Err(gate.stop()));
    }
    let port = settings.port_text.as_deref().and_then(curl_port)?;
    let post_max_time = post_max_time(std::env::var_os(POST_MAX_TIME).as_deref())?;
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock, would_refuse, TOOL) {
        return Some(Err(stop));
    }
    settings.port = Some(port);
    Some(
        settings
            .client(would_refuse)
            .map(|mut c| {
                c.post_max_time = post_max_time;
                c
            })
            .map_err(|e| {
                out::stderr_line(&format!("Error: {e}"));
                Exit::Error
            }),
    )
}

/// The shell's setting for its form and JSON POSTs' `--max-time`.
pub const POST_MAX_TIME: &str = "_K2_POST_MAX_TIME";

/// `${_K2_POST_MAX_TIME:-30}` as curl's `--max-time` reads it: unset or
/// empty is the default, digits with at most one `.` are seconds, and 0 is
/// no limit. `None`: a value only curl knows what to do with, so the shell
/// answers the command.
pub fn post_max_time(v: Option<&std::ffi::OsStr>) -> Option<Option<std::time::Duration>> {
    let Some(v) = v.filter(|v| !v.is_empty()) else {
        return Some(None);
    };
    let t = v.to_str()?;
    let plain = t.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && t.bytes().filter(|b| *b == b'.').count() <= 1
        && t.bytes().any(|b| b.is_ascii_digit());
    if !plain {
        return None;
    }
    let secs: f64 = t.parse().ok()?;
    Some(Some(if secs == 0.0 {
        std::time::Duration::from_secs(u64::from(u32::MAX))
    } else {
        std::time::Duration::from_secs_f64(secs)
    }))
}

/// `_envn_into VAR NAME`: the first of the K5_, K2_ and K2SO_ names that is
/// set and not empty, or `None` (also for a value that is not UTF-8).
pub fn envn(names: &'static [&'static str]) -> Option<String> {
    env_names::first_nonempty_os_in(names, |n| std::env::var_os(n))
        .and_then(|(_, v)| v.into_string().ok())
}
