//! Shared `AgentSignal` → PTY-bytes formatter.
//!
//! Used by:
//!   - `providers::DaemonInjectProvider` (indirectly — egress
//!     formats signals before calling the provider, but this is
//!     the fallback shape the daemon itself uses)
//!   - `awareness_ws::handle_sessions_spawn` drain loop, where we
//!     read pending-live signals off disk and inject them as
//!     the target session boots.
//!
//! Keeps the formatting decision in one place so pending-drain
//! injections look identical to live injections.

use k2_core::awareness::{AgentAddress, AgentSignal, SignalKind};

/// Render an AgentSignal to the body bytes that get written to the
/// target's PTY. Formats each SignalKind variant as a short
/// `[from] body` line — **without** a trailing newline or carriage
/// return. The submit trigger (Enter, i.e. `\r`) must be written by
/// the caller as a separate syscall after a brief settle delay; a
/// combined "body + \r" single write is interpreted by TUI input
/// widgets (claude code, codex, gemini) as a multi-line paste and
/// inserted as-is rather than as "type then submit." See
/// `heartbeat_launch::run_inject` for the two-phase reference
/// pattern. Phase 4+ per-harness integrations can replace this with
/// ANSI-formatted, agent-speak, or structured output.
pub fn inject_bytes(signal: &AgentSignal) -> String {
    let body = match &signal.kind {
        SignalKind::Msg { text } => text.clone(),
        SignalKind::Status { text } => format!("[status] {text}"),
        SignalKind::Presence { state } => format!("[presence {state:?}]"),
        SignalKind::Reservation { paths, action } => {
            format!("[{action:?}] {}", paths.join(", "))
        }
        SignalKind::TaskLifecycle { phase, task_ref } => {
            let r = task_ref.as_deref().unwrap_or("");
            format!("[task {phase:?}] {r}")
        }
        SignalKind::Custom { kind, payload } => {
            format!("[{kind}] {payload}")
        }
        // SignalKind is #[non_exhaustive] so a catchall is required
        // by the compiler; unknown variants land as a marker line
        // that's easy to eyeball in a target's terminal history.
        _ => "[unknown signal kind]".to_string(),
    };
    let from = match &signal.from {
        AgentAddress::Agent { name, .. } => name.clone(),
        AgentAddress::Workspace { .. } => "workspace".into(),
        AgentAddress::Broadcast => "broadcast".into(),
        _ => "unknown".into(),
    };
    format!("[{from}] {body}")
}
