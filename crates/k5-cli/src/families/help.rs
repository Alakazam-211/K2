//! `k5 help` (also `--help`, `-h`): the shell CLI's row, and the helpers
//! the help family's modules share.
//!
//! The help family's rows (`help`, `help-advanced`, `help-internal`,
//! `help-deprecated`) stay the shell's (`shell_family!`): the front
//! already prints, while a row is the shell's, every form bash only prints
//! a page for ([`crate::front::route`]): `k2`, `help` and `--help` (the
//! daily tier), `-h` and `help <word>` for a word `cmd_help` has no arm
//! for (the daily tier too), `help deprecated`, `help-deprecated` and
//! `help chat` (pages bash puts together), and `help <word>`,
//! `help-advanced` and `help-internal` (a page each,
//! [`crate::text::HELP_FORMS`]). A `Native` row would lose them: the
//! front sends a `Native` row's line to the family (so `help-advanced`
//! would no longer be its page route, and `front`'s tests pin that it
//! is), and a line with `-h` or `--help` in it only to a page from that
//! list, which `--help`, `-h` and `--help <word>` for a word with no page
//! aren't on. So the rows turn when the front sends those lines to their
//! pages first. What the shell still answers itself: `help <word> <more
//! words>` (`cmd_help` reads only the first) and a tier word with words
//! after it (its arm reads none).
//!
//! [`shell_prelude`] and [`show`] are for the families that print a page
//! themselves (`--schema`, `explain`).

use crate::exit::Exit;
use crate::offline::{self, Env, Prelude};
use crate::out;
use crate::pages::Page;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["help", "--help", "-h"];
/// The shell CLI answers every form but [`VERBS`]'.
pub const OWNER: crate::route::Owner = crate::route::Owner::Bash;
/// `help remote` is the binary's.
pub const VERBS: &[crate::route::Verb] = &[crate::route::Verb {
    words: &["remote"],
    owner: crate::route::Owner::Native,
}];

/// `k5 help remote` (and words after it, which `cmd_help` never reads):
/// after the connection gate (the virtual desktop's guard passes `help`),
/// `_remote_usage` on stdout, exit 0. `help` is an open tool, so the
/// session checks pass it. `None`: the shell answers any other line.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let words: Vec<&str> = args.iter().map(|a| a.to_str()).collect::<Option<_>>()?;
    if !matches!(words.as_slice(), ["help", "remote", ..]) {
        return None;
    }
    if !crate::python::family_native(crate::python::here()) {
        return None;
    }
    let settings = match crate::daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    // `help` is an offline verb for the virtual desktop's guard, so with
    // no token only the connection gate speaks.
    if crate::prelude::gate(&settings).is_some() {
        return Some(crate::prelude::Gate::CannotConnect.stop());
    }
    Some(show(&Page {
        text: super::remote::usage().into(),
        stderr: false,
        exit: Exit::Ok,
        lead: None,
    }))
}

/// The shell's prelude for a page a family prints itself, as the front
/// reads it for its own pages (`main.rs`'s `page`): `None` when a setting
/// bash reads at start, a missing program or the like hands the command
/// to the shell.
///
/// Shared by the families that print a page (schema, explain); it sits
/// here until the gardener moves it beside [`crate::pages`].
pub fn shell_prelude() -> Option<(Env, Prelude)> {
    let env = Env::current();
    let p = offline::shell_pwd(&env)
        .and_then(|cwd| offline::prelude(&env, &cwd))
        .ok()?;
    Some((env, p))
}

/// Prints `page` as the front prints its own pages: the lead line on
/// stderr (as `echo ... >&2` under `set -e`, a stderr that can't be
/// written ends the run before the page), then the text on stderr (as
/// `cat >&2`, a failed write is no error) or on stdout.
pub fn show(page: &Page) -> Exit {
    if let Some(lead) = &page.lead {
        if let Err(err) = out::write_flushed(&mut std::io::stderr().lock(), lead.as_bytes()) {
            return out::after_stdout_error(&err);
        }
    }
    if page.stderr {
        let _ = out::write_flushed(&mut std::io::stderr().lock(), page.text.as_bytes());
        return page.exit;
    }
    match out::stdout(&page.text) {
        Ok(()) => page.exit,
        Err(err) => out::after_stdout_error(&err),
    }
}
