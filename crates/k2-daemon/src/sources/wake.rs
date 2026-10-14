//! Trigger → wake (contract §9). Owner: B2. The engine calls
//! [`handle_transitions`] after every state commit with hard transitions,
//! [`handle_unmute`] when a mute or simulate ends, and
//! [`redeliver_pending`] once at boot + 60 s.
//!
//! C0 skeleton: nothing fires yet.

use k2_core::sources::{SourceRow, Transition, WakeOutcome, WakeRequest, Waker};

/// The daemon's [`Waker`]: `workspace_msg::deliver_live_with_via` with
/// `via = "source"`, falling back to the agent's inbox.
pub struct DaemonWaker;

impl Waker for DaemonWaker {
    /// C0 skeleton: undeliverable (B2 replaces it).
    fn deliver(&self, req: &WakeRequest) -> WakeOutcome {
        let _ = req;
        WakeOutcome::Undeliverable { code: "not_built".into() }
    }
}

/// Evaluate every active trigger on `source` against `ts`, write fire rows,
/// queue deliveries.
pub fn handle_transitions(source: &SourceRow, ts: &[Transition], now: i64) {
    let _ = (source, ts, now);
}

/// A mute or simulate on `source` ended: fire once for an incident that is
/// open now and never fired.
pub fn handle_unmute(source: &SourceRow, now: i64) {
    let _ = (source, now);
}

/// Re-queue `pending` fires older than 60 s whose incident is still open.
pub fn redeliver_pending(now: i64) {
    let _ = now;
}
