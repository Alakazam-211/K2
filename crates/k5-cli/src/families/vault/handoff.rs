//! `_vault_handoff` (`cli/src/vault.sh`): send, incoming, sent, accept-from, send-allow, ask, asks and unsend.
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
