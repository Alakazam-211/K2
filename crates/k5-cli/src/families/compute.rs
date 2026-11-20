//! `k5 compute`: the family's one entry. Its row in [`crate::route::TABLE`]
//! is [`WORDS`], [`OWNER`] and [`VERBS`]; [`run`] hands each native verb to
//! the side that answers it:
//!
//! - [`reads`]: the verbs that only read (`nodes`, `ls`, `info`, `queue`,
//!   `jobs`, `get`, `status`, `grants`, `receipt`, `events`, `stats`,
//!   `usage`, `logs`, `preview` and `placement` with no change, and
//!   `rightsize`).
//! - [`writes`]: the verbs that write, wait or move files (`cancel`,
//!   `wait`, `fetch` and `grant`).
//! - [`admin`]: the admin verbs (`revoke`, `preview on|off`, `placement
//!   v1|v2`, `access`, `operate`, `manage`, `retier`, `tier`, `move`,
//!   `cache`, the `node` subverbs made native so far, and the setup verbs
//!   `allow-nodes`, `git-access`, `lanes`, `local` and `attach`, and
//!   `compute` alone). [`follow`] is `_compute_follow`.
//! - [`more`]: `retry`, `provenance`, `verify`, and `roster`, `seats`,
//!   `trust` and `owners`.
//! - [`submit`]: `run`, `test` and `prewarm`.
//!
//! Every other line is the shell's. The modules below
//! are shared: [`pyrt`] (the Python operations the shell's readers use),
//! [`render`] (`_compute_render`), [`lines`] (where the shell's Python
//! lines are) and `reads::call` (`_compute_call`).

pub mod admin;
pub mod events;
pub mod follow;
pub mod lines;
pub mod logs;
pub mod more;
pub mod pyrt;
pub mod reads;
pub mod render;
pub mod status;
pub mod submit;
pub mod writes;

use crate::exit::Exit;
use crate::route::{Owner, Verb};
use std::ffi::OsString;

/// The router arm's words.
pub const WORDS: &[&str] = &["compute"];
/// The binary's: every arm of `cmd_compute` (`cli/src/compute-command.sh`)
/// is native, and `compute` alone too. A form [`run`] doesn't take still
/// goes to the shell CLI.
pub const OWNER: Owner = Owner::Native;
/// None: [`OWNER`] answers every verb.
pub const VERBS: &[Verb] = &[];

/// Whether this family's code answers `verb` (a second word).
pub fn answers(verb: &str) -> bool {
    [
        reads::ANSWERS,
        writes::ANSWERS,
        admin::ANSWERS,
        more::ANSWERS,
        submit::ANSWERS,
    ]
    .iter()
    .any(|a| a.contains(&verb))
}

/// Runs `args` (the whole command line after `argv[0]`). `None`: the
/// shell CLI answers it. Each side hands back, before it prints or sends
/// anything, a line it doesn't answer: `preview` and `placement` with no
/// word, `status` or a bad word are [`reads`]', `on|off` and `v1|v2` are
/// [`admin`]'s.
pub fn run(args: &[OsString]) -> Option<Exit> {
    reads::run(args)
        .or_else(|| writes::run(args))
        .or_else(|| admin::run(args))
        .or_else(|| more::run(args))
        .or_else(|| submit::run(args))
}
