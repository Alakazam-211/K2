//! The verbs `cmd_vault`'s own flag parser reads (`cli/src/migrate.sh`): see [`super`].
//!
//! Nothing here is native yet: [`native`] hands every form to the shell.

use super::Ctx;
use crate::exit::Exit;

/// Whether the binary answers `vault <sub> <rest>` (no I/O).
pub fn native(_sub: &str, _rest: &[&str]) -> bool {
    false
}

/// The verb. Called only for a form [`native`] takes.
pub fn run(_ctx: &mut Ctx, _sub: &str, _rest: &[&str]) -> Exit {
    Exit::Error
}
