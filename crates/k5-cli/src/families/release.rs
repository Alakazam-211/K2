//! `k5 release [paths…] --agent <name>`: [`super::reserve`] answers it,
//! as the shell CLI's `cmd_release` shares `_reserve_args` with
//! `cmd_reserve` (`cli/src/usage.sh`).

use crate::exit::Exit;
use std::ffi::OsString;

/// The router arm's words: this family's row of [`crate::route::TABLE`].
pub const WORDS: &[&str] = &["release"];
/// The binary answers every form; [`run`] hands the ones
/// [`super::reserve`] names to the shell.
pub const OWNER: crate::route::Owner = crate::route::Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[crate::route::Verb] = &[];

/// `k5 release ...`: [`super::reserve::run_verb`].
pub fn run(args: &[OsString]) -> Option<Exit> {
    super::reserve::run_verb(super::reserve::Verb::Release, args)
}
