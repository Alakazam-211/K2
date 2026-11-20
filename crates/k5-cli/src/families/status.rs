//! `k5 status`: retired: `fail_deprecated`, as the shell CLI's `status` arm
//! (`cli/src/router.sh`). [`super::commit`] answers it with the router's
//! other small arms.

use crate::exit::Exit;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["status"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 status ...`: [`super::commit::run`].
pub fn run(args: &[OsString]) -> Option<Exit> {
    super::commit::run(args)
}
