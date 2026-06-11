//! Awareness Bus ingress — signal enrichment + routing.
//!
//! E5 of Phase 3. When a `Frame::AgentSignal` emerges from the
//! line-mux inside a session's reader loop, it arrives with
//! placeholder fields (APC payloads don't know the sender or the
//! session — those are computed at ingress time from session
//! context). This module enriches the signal with the information
//! line_mux can't know, then hands it to `egress::deliver`.
//!
//! Called from `terminal::session_stream_pty::reader_loop` once per
//! AgentSignal frame. Keeps line_mux free of egress coupling —
//! line_mux just produces Frames; ingress is the site where Frames
//! become Bus activity.
//!
//! The inbox_root is resolved lazily from a process-wide default
//! (`~/.k2so/awareness/inbox/`) so callers in different workspaces
//! don't have to thread the path through. Phase 4 adds per-workspace
//! override via an ambient provider.

use std::path::PathBuf;
use std::sync::OnceLock;

use parking_lot::Mutex;

use crate::awareness::{egress, AgentAddress, AgentSignal, WorkspaceId};
use crate::awareness::DeliveryReport;
use crate::session::SessionId;

/// Override for the default inbox root. Daemon sets this at startup
/// so signal deliveries for cross-session targets land in the right
/// project directory. Unset → `~/.k2so/awareness/inbox/` default.
static INBOX_ROOT_OVERRIDE: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

fn inbox_root_slot() -> &'static Mutex<Option<PathBuf>> {
    INBOX_ROOT_OVERRIDE.get_or_init(|| Mutex::new(None))
}

/// Register a process-wide default inbox root. Idempotent — second
/// call overwrites. Useful for daemon startup and for tests that
/// point the ingress path at a tempdir.
pub fn set_inbox_root(root: PathBuf) {
    *inbox_root_slot().lock() = Some(root);
}

/// Test helper — unset any prior override.
#[cfg(any(test, feature = "test-util"))]
pub fn clear_inbox_root_for_tests() {
    *inbox_root_slot().lock() = None;
}

/// Current effective inbox root. Override → default
/// `~/.k2so/awareness/inbox/`. If home can't be resolved (headless
/// container without $HOME), falls back to `/tmp/.k2so-awareness-inbox`
/// so the pipeline still works — tests should prefer
/// `set_inbox_root` over relying on the fallback.
pub fn inbox_root() -> PathBuf {
    if let Some(root) = inbox_root_slot().lock().clone() {
        return root;
    }
    match dirs::home_dir() {
        Some(home) => home.join(".k2so/awareness/inbox"),
        None => PathBuf::from("/tmp/.k2so-awareness-inbox"),
    }
}

/// Route a signal that emerged from inside a live session. Called
/// by the PTY reader loop when line_mux produces `Frame::AgentSignal`.
///
/// Enrichment:
///   - `signal.session` is set to the session id
///   - `signal.from` is filled with the session's agent name +
///     workspace if `set_session_workspace` has populated one
///
/// Then the signal is handed to `egress::deliver`.
pub fn from_session(
    session_id: SessionId,
    mut signal: AgentSignal,
    session_agent_name: Option<&str>,
    session_workspace: Option<&WorkspaceId>,
) -> DeliveryReport {
    if signal.session.is_none() {
        signal.session = Some(session_id);
    }
    // Only overwrite `from` if it's the APC-level placeholder (Broadcast
    // with no agent info). Don't clobber an explicit sender identity
    // that might have been set elsewhere.
    if matches!(signal.from, AgentAddress::Broadcast) {
        if let Some(name) = session_agent_name {
            signal.from = AgentAddress::Agent {
                workspace: session_workspace
                    .cloned()
                    .unwrap_or_else(|| WorkspaceId(String::new())),
                name: name.to_string(),
            };
        }
    }
    // Enrich `to` with the session's workspace when the target has an
    // empty workspace (APC payloads can't know their own workspace;
    // the line_mux dispatcher puts an empty WorkspaceId as a
    // placeholder in `k2so:msg` parsing).
    if let AgentAddress::Agent { workspace, .. } = &mut signal.to {
        if workspace.0.is_empty() {
            if let Some(ws) = session_workspace {
                *workspace = ws.clone();
            }
        }
    }

    egress::deliver(&signal, &inbox_root())
}

/// Route a signal emitted from outside any session (e.g. the
/// `k2so msg` CLI). Caller supplies the workspace + from-agent
/// directly since there's no session to infer from.
pub fn from_cli(signal: AgentSignal) -> DeliveryReport {
    egress::deliver(&signal, &inbox_root())
}
