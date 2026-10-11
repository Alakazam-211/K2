//! Process-wide "this daemon has handed off" flag (runtime stage H).
//!
//! When a daemon hands its listening sockets to a successor
//! (`k2-daemon/src/handoff`), it stays alive only to pump the connections
//! it already spliced. From that moment it must start nothing: no agent
//! PTY (the successor resumes those — two would double-resume a session on
//! the same tree), no frpc (the successor adopts the running one), no
//! published service. The spawn choke points in k2-core
//! ([`crate::terminal::daemon_pty::DaemonPtySession::spawn`], the tunnel
//! connector) check [`draining`] and refuse.

use std::sync::atomic::{AtomicBool, Ordering};

static DRAINING: AtomicBool = AtomicBool::new(false);

/// The refusal text every guarded spawn returns.
pub const REFUSAL: &str = "this daemon has handed off to its successor and only drains live \
     connections — refusing to start anything new";

/// Enter drain-only mode. One-way: a daemon never leaves it.
pub fn set_draining() {
    DRAINING.store(true, Ordering::SeqCst);
}

/// True once [`set_draining`] ran.
pub fn draining() -> bool {
    DRAINING.load(Ordering::SeqCst)
}

/// `Err(REFUSAL)` while draining.
pub fn guard() -> Result<(), String> {
    if draining() {
        Err(REFUSAL.to_string())
    } else {
        Ok(())
    }
}

/// Test-only: leave drain mode so later tests in the binary start clean.
#[doc(hidden)]
pub fn reset_for_tests() {
    DRAINING.store(false, Ordering::SeqCst);
}
