//! Windows stand-in for the vault's ssh-agent (`ssh_agent.rs` is a Unix
//! socket server). Pending Windows parity: OpenSSH for Windows reads a
//! named pipe from `SSH_AUTH_SOCK`, so the Windows agent is a named-pipe
//! server with the same protocol, limits and audit. Until it lands an
//! `agent` item is not delivered: the session's spawn lists it as skipped
//! ("agent socket (unsupported)"), and nothing else changes.

use std::path::{Path, PathBuf};

use k2_vault::{LocalVault, Requester};

/// Always `Unsupported` on Windows for now (see the module doc).
pub fn start(_vault: &LocalVault, _who: Requester, _tag: &str, _dir: &Path) -> std::io::Result<PathBuf> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "the vault ssh-agent isn't on Windows yet"))
}

/// No agent is ever started, so there is nothing to stop.
pub fn stop_dir(_dir: &Path) {}

/// No agent sockets are ever recorded on Windows.
pub fn boot_sweep(_vault: &LocalVault, _kept: &[(String, PathBuf)]) {}

/// No socket serves any dir on Windows.
pub fn socket_for(_dir: &Path) -> Option<PathBuf> {
    None
}
