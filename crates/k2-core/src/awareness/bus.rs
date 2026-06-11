//! Awareness Bus ambient singleton — in-memory fan-out of `AgentSignal`s.
//!
//! E1 of Phase 3. Bare minimum: a `tokio::broadcast::Sender<AgentSignal>`
//! hidden behind a `OnceLock`, with `publish()` + `subscribe()` free
//! functions. No I/O. No inbox. No activity_feed write. No delivery-
//! mode routing. Those arrive in E2, E4, and E5 respectively.
//!
//! The bus only handles the hot-path case: sender publishes, any
//! subscribed receivers get a copy. Single process, in-memory. Phase 3
//! E7 wires the daemon's WS subscribe endpoint into this singleton so
//! cross-process consumers (CLI, Tauri UI) can attach through HTTP.
//!
//! Matches `session::entry::SessionEntry`'s broadcast capacity (256)
//! and the daemon's existing `/events` channel. Slow subscribers get
//! `RecvError::Lagged(n)` — the handler loop logs and continues per
//! the PRD-locked drop-oldest backpressure policy (2026-04-19 Q4).

use std::sync::OnceLock;

use tokio::sync::broadcast;

use crate::awareness::AgentSignal;

/// Broadcast channel capacity. Matches `SessionEntry::BROADCAST_CAP`
/// and the daemon's `/events` endpoint — consistency across the
/// workspace. Slow subscribers get `Lagged` once the channel fills;
/// live PTY-inject targets (from E4) are paced by kernel write
/// buffers and don't hit this limit.
pub const BUS_CAP: usize = 256;

static BUS: OnceLock<broadcast::Sender<AgentSignal>> = OnceLock::new();

/// Internal accessor — lazy-initializes the sender on first call.
fn sender() -> &'static broadcast::Sender<AgentSignal> {
    BUS.get_or_init(|| broadcast::channel(BUS_CAP).0)
}

/// Publish a signal to every currently-subscribed receiver. Silent
/// when there are no receivers — matches the daemon's `/events`
/// semantics ("emitters fire whether or not a UI is listening, and
/// that's the whole point").
///
/// This is *only* the in-memory broadcast side. E4's `egress::deliver`
/// is what callers will actually use in production — it composes
/// this broadcast with the Inbox + PTY-inject + activity_feed paths.
/// Direct `bus::publish()` is for subscribers that want the raw
/// signal stream (daemon's `/cli/awareness/subscribe` WS endpoint,
/// debugger CLIs).
pub fn publish(signal: AgentSignal) {
    let _ = sender().send(signal);
}

/// Subscribe to the live bus stream. Returns a new receiver that
/// sees every signal `publish()`-ed AFTER this call. Missed signals
/// (published before subscribe) must be recovered through other
/// means — e.g. activity_feed query, inbox file scan, session
/// replay ring. The bus is strictly a hot path.
pub fn subscribe() -> broadcast::Receiver<AgentSignal> {
    sender().subscribe()
}

/// Count of active receivers. Useful for dashboards / telemetry;
/// the bus itself doesn't care whether anyone is listening.
pub fn subscriber_count() -> usize {
    sender().receiver_count()
}
