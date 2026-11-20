//! `k5 reminders`: `remind list` by another word, as the shell CLI's
//! `reminders` arm (`cli/src/router.sh`) runs it. [`super::remind`]
//! answers it.

use crate::exit::Exit;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["reminders"];
/// The binary answers every form; [`run`] hands the ones it names to
/// the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 reminders ...`: [`super::remind::run`], which reads `reminders` as
/// `remind list`.
pub fn run(args: &[OsString]) -> Option<Exit> {
    super::remind::run(args)
}
