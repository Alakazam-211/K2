//! `k5 remote`: this computer's sign-in to another server, as the shell
//! CLI's `cmd_remote` answers it (`cli/src/remote-logout.sh`): `remote`
//! alone prints the usage on stderr (exit 2), `help` on stdout (exit 0),
//! an unknown verb its line and the usage (exit 2). `remote` skips the
//! connection gate; the virtual desktop's guard and the locked
//! `connect-account` tool's checks still stand.
//!
//! `logout` is native too ([`logout`]): one POST to the other server
//! over the shared TLS, then the saved copies of that token go.
//!
//! Handed to the shell ([`run`] returns `None`): any `-h` or `--help`
//! (the front's), a word that is not UTF-8, `HOOK_SOCK` set with a
//! `CODEX_` variable exported, a `python3` the binary can't tell, and
//! the `logout` cases [`logout::run`] names.

pub mod logout;

use crate::daemon;
use crate::exit::Exit;
use crate::out;
use crate::passport;
use crate::prelude::{self, Gate};
use crate::python;
use k2_client::{cli_name, product_name};
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["remote"];
/// The binary answers every form.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// The tool's name in the shell's policy.
pub const TOOL: &str = "connect-account";

/// `_remote_usage`.
pub fn usage() -> String {
    format!(
        "{c} remote \u{2014} this computer's sign-in to another {p} server

  {c} remote logout --host HOST [--token-stdin]
      End this computer's session on HOST (POST /cli/auth/logout).
      The token sent is the one `{c} talk <ws>@HOST` sends
      ($K2_REMOTE_TOKEN, then connect-tokens.json, then the
      keychain), or with --token-stdin one read from stdin. A token
      never goes on the command line. HOST is a name (https) or a URL;
      plain http:// only to this computer.
      After HOST answers, the saved copies of that token for HOST go
      too (connect-tokens.json and the keychain). A saved token that
      differs is kept, and so are the server list and a remembered
      password.

  What logout can't do:
    It can't revoke an owner token: an owner token has no session to
    end (a restart of that server's daemon does).
    Success can't prove a session existed: the server answers 200
    whether or not it had one.

  Exit: 0 signed out, 1 no answer or an unexpected one, 2 usage,
  3 HOST refused (nothing is removed here).
",
        c = cli_name!(),
        p = product_name!()
    )
}

/// `k5 remote ...`: `args` is the whole command line after `argv[0]`.
/// `None`: the shell CLI answers it.
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
    let sub = words.first().copied().unwrap_or("");
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
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
    if let Some(Gate::VirtualDesktop) = prelude::gate(&settings) {
        return Some(Gate::VirtualDesktop.stop());
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    if sub == "logout" {
        let home = match crate::home::k2_home() {
            Ok(h) => h,
            Err(_) => return None,
        };
        return logout::run(&words[1..], &home);
    }
    let (stdout, stderr, exit) = lines(sub);
    if !stdout.is_empty() {
        if let Err(e) = out::stdout(&stdout) {
            return Some(out::after_stdout_error(&e));
        }
    }
    if !stderr.is_empty() {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), stderr.as_bytes());
    }
    Some(exit)
}

/// What `cmd_remote` prints for a verb that is not `logout`: stdout,
/// stderr and the exit.
pub fn lines(sub: &str) -> (String, String, Exit) {
    match sub {
        "help" | "-h" | "--help" => (usage(), String::new(), Exit::Ok),
        "" => (String::new(), usage(), Exit::Usage),
        other => (
            String::new(),
            format!(
                "{} remote: unknown verb '{other}'\n{}",
                cli_name!(),
                usage()
            ),
            Exit::Usage,
        ),
    }
}
