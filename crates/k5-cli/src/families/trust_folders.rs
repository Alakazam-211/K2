//! `k5 trust-folders`: `cli_request_post /cli/trust-folders`, as the shell CLI's `trust-folders` arm
//! (`cli/src/router.sh`). [`super::commit`] answers it with the router's
//! other small arms.

use crate::exit::Exit;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["trust-folders"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 trust-folders ...`: [`super::commit::run`].
pub fn run(args: &[OsString]) -> Option<Exit> {
    super::commit::run(args)
}
