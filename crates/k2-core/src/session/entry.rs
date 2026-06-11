//! `SessionEntry` — one session's live broadcast + replay ring.
//!
//! Producer (Phase 2's `session_stream_pty` reader thread, later the
//! daemon's session spawn path) calls `publish(frame)`. Every
//! currently-subscribed receiver gets a copy via the tokio broadcast
//! channel; late subscribers can drain the replay ring to catch up
//! before hooking into live.
//!
//! Layers in the PRD at `.k2so/prds/session-stream-and-awareness-bus.md`
//! §"Session persistence model":
//!   - Live broadcast:   `tokio::sync::broadcast::Sender<Frame>`
//!   - Replay ring:      bounded `VecDeque<Frame>` (front-pop on overflow)
//!   - Archive log:      deferred to Phase 3
//!
//! `SessionEntry` holds only the first two. The archive NDJSON
//! writer task attaches as just another subscriber in Phase 3.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::sync::broadcast;

use crate::session::bytes_ring::{BytesRing, RingChunk};
use crate::session::Frame;

/// Replay-ring capacity. Originally 1000 per the PRD default, but
/// that was tuned for live-spawned sessions where subscribers attach
/// within a few hundred ms and miss at most a handful of bytes. It
/// breaks down for `claude --resume <id>` (and similar harness
/// resumes) where the child process replays the entire prior
/// conversation to the PTY in a burst of tens of thousands of bytes.
/// LineMux emits a fresh Frame::Text on every SGR change, and
/// Claude's markdown output is heavily styled — a medium
/// conversation can produce ~5-10 frames per visible line, and a
/// resume burst easily overflows a 1000-cap ring before the Tauri
/// renderer has opened its WebSocket. The user-visible symptom is
/// "resume shows only the last few lines of the conversation."
///
/// Bumped to 50_000 so typical resumes fit end-to-end. Memory cost
/// per session: ~50_000 × avg_frame_bytes (~100-200 B) ≈ 5-10 MB,
/// bounded and deallocated when the session ends.
///
/// The permanent fix is to replay the on-disk NDJSON archive
/// (`session::archive`) for late subscribers so we're not
/// memory-bound at all, but that's a larger change. This constant
/// bump is the minimal hotfix.
pub const REPLAY_CAP: usize = 50_000;

/// Broadcast-channel capacity — matches the daemon's existing
/// `/events` endpoint (crates/k2so-daemon/src/events.rs:48). Slow
/// receivers get `RecvError::Lagged(n)` once the channel's internal
/// buffer fills; they can then catch up from the replay ring.
pub const BROADCAST_CAP: usize = 256;

/// Broadcast capacity for the raw-byte channel (Canvas Plan Phase 2).
/// A lagged byte subscriber falls back to the byte ring + on-disk
/// archive for continuity; the broadcast is only for the live tail.
/// Higher than the frame broadcast because byte chunks arrive more
/// frequently (one per PTY read vs one per logical Frame burst).
pub const BYTES_BROADCAST_CAP: usize = 1024;

/// Everything a session needs to fan frames to N consumers.
#[derive(Debug)]
pub struct SessionEntry {
    /// Live broadcast. Tokio's broadcast drops oldest for slow
    /// receivers by default — exactly the backpressure policy the
    /// PRD locks (2026-04-19 Q4 resolution).
    pub tx: broadcast::Sender<Frame>,
    /// Bounded replay buffer for late-joining subscribers.
    pub replay: Arc<Mutex<VecDeque<Frame>>>,
    /// Max size of the replay ring. Trimmed front-first on overflow.
    replay_cap: usize,
    /// Canvas Plan Phase 2: live broadcast of raw PTY bytes. Every
    /// chunk read from the PTY master is published here (in
    /// addition to being driven through LineMux into the Frame
    /// channel). Subscribers that want pixel-perfect reflow (a
    /// local alacritty_terminal::Term) read from this channel +
    /// the `bytes_ring` + on-disk byte archive.
    pub bytes_tx: broadcast::Sender<Arc<[u8]>>,
    /// Canvas Plan Phase 2: bounded in-memory byte buffer with
    /// offset tracking. Sized by `BYTES_RING_CAP`; see
    /// `bytes_ring::BytesRing` for eviction semantics.
    pub bytes_ring: Arc<Mutex<BytesRing>>,
    /// Optional agent name the session belongs to. Set when the
    /// session is spawned for a specific agent (0.34.0 Phase 3
    /// onward). Anonymous sessions (one-off debugging, test
    /// fixtures) leave this `None`; roster queries skip them.
    agent_name: Mutex<Option<String>>,
    /// Monotonic time the entry was created. Used by the harness
    /// watchdog to avoid SIGTERM-ing sessions that just spawned
    /// (some harnesses take several seconds before emitting their
    /// first frame). Read-only after construction.
    created_at: Instant,
    /// Monotonic time of the most recent `publish()` call. Updated
    /// atomically on every frame emission. Watchdogs read this to
    /// decide whether a session is wedged.
    last_frame_at: Mutex<Instant>,
}

impl SessionEntry {
    /// Build a fresh entry with empty replay ring and default
    /// capacities.
    pub fn new() -> Self {
        Self::with_replay_cap(REPLAY_CAP)
    }

    /// Build with a custom replay cap. Mostly for tests.
    pub fn with_replay_cap(replay_cap: usize) -> Self {
        assert!(replay_cap >= 1, "SessionEntry replay_cap must be >= 1");
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        let (bytes_tx, _) = broadcast::channel(BYTES_BROADCAST_CAP);
        let now = Instant::now();
        Self {
            tx,
            replay: Arc::new(Mutex::new(VecDeque::with_capacity(replay_cap))),
            replay_cap,
            bytes_tx,
            bytes_ring: Arc::new(Mutex::new(BytesRing::new())),
            agent_name: Mutex::new(None),
            created_at: now,
            last_frame_at: Mutex::new(now),
        }
    }

    /// Tag this session with the agent it represents. Idempotent —
    /// second call overwrites; useful if a session rebinds to a
    /// different agent (rare but not forbidden).
    pub fn set_agent_name(&self, name: impl Into<String>) {
        *self.agent_name.lock() = Some(name.into());
    }

    /// Read the current agent-name binding, if any.
    pub fn agent_name(&self) -> Option<String> {
        self.agent_name.lock().clone()
    }

    /// Publish a session-content frame. Durably appended to the
    /// replay ring, broadcast to live subscribers, and bumps
    /// `last_frame_at` so the watchdog sees this as harness
    /// activity and resets its idle timer.
    ///
    /// Use this for every frame that originates from the harness
    /// (PTY output, agent signals, semantic events lifted from
    /// harness protocols). For observer-emitted frames like
    /// watchdog escalations, use `publish_meta` so the watchdog
    /// doesn't inadvertently reset its own idle timer.
    pub fn publish(&self, frame: Frame) {
        self.publish_inner(frame, true);
    }

    /// Publish a meta frame without touching `last_frame_at`.
    /// Intended for watchdog escalations + other observer-emitted
    /// frames that describe session state rather than carry
    /// harness output. Subscribers still see the frame in the
    /// same stream; only the idle-timer bump is suppressed.
    ///
    /// Primitive rule: anything the WATCHDOG emits is meta;
    /// anything the HARNESS emits is content. Keep the two lanes
    /// separate so `idle_for(now)` always answers "how long since
    /// the harness last produced" rather than "how long since any
    /// frame was published."
    pub fn publish_meta(&self, frame: Frame) {
        self.publish_inner(frame, false);
    }

    fn publish_inner(&self, frame: Frame, bump_activity: bool) {
        {
            let mut replay = self.replay.lock();
            replay.push_back(frame.clone());
            while replay.len() > self.replay_cap {
                replay.pop_front();
            }
        }
        let _ = self.tx.send(frame);
        if bump_activity {
            *self.last_frame_at.lock() = Instant::now();
        }
    }

    /// Subscribe to the live stream. The returned receiver sees
    /// frames published AFTER the subscribe call — call
    /// `replay_snapshot()` to catch up on prior frames before
    /// draining this.
    pub fn subscribe(&self) -> broadcast::Receiver<Frame> {
        self.tx.subscribe()
    }

    /// Canvas Plan Phase 2: publish a raw-byte chunk. Appends to the
    /// byte ring (with offset tracking) AND broadcasts to live byte
    /// subscribers. Returns the absolute byte offset the chunk lands
    /// at (i.e. the offset of its first byte in the Session's byte
    /// stream).
    ///
    /// Called from `session_stream_pty::reader_loop` once per PTY
    /// read, in tandem with the existing Frame publish path. Both
    /// taps see the same bytes in the same order.
    pub fn publish_bytes(&self, data: Arc<[u8]>) -> u64 {
        let offset = self.bytes_ring.lock().push(Arc::clone(&data));
        let _ = self.bytes_tx.send(data);
        offset
    }

    /// Subscribe to the live byte broadcast. New chunks published
    /// AFTER this call are delivered. For historical bytes, call
    /// `bytes_snapshot_from(offset)` first + read the on-disk
    /// byte archive when the ring has already evicted past the
    /// requested offset.
    pub fn bytes_subscribe(&self) -> broadcast::Receiver<Arc<[u8]>> {
        self.bytes_tx.subscribe()
    }

    /// Snapshot of the byte ring starting at (or just before)
    /// `from_offset`. Each returned chunk is tagged with its
    /// absolute offset so the caller can skip partial-chunk
    /// prefixes. Returns empty when `from_offset` is past the
    /// ring's current back.
    pub fn bytes_snapshot_from(&self, from_offset: u64) -> Vec<RingChunk> {
        self.bytes_ring.lock().snapshot_from(from_offset)
    }

    /// Current byte-stream cursor pair: (front_offset, back_offset).
    /// `front` is the earliest in-memory byte; `back` is the next
    /// byte to be written. `[0, front)` is only available from the
    /// on-disk archive.
    pub fn bytes_offsets(&self) -> (u64, u64) {
        let r = self.bytes_ring.lock();
        (r.front_offset(), r.back_offset())
    }

    /// Current live-byte subscriber count — diagnostic.
    pub fn bytes_subscriber_count(&self) -> usize {
        self.bytes_tx.receiver_count()
    }

    /// Snapshot of the replay ring — clones up to `replay_cap`
    /// frames. Intended for the "subscriber connects, gets
    /// last-N, then hooks into live" flow the WS endpoint
    /// (Phase 2 D4) will implement.
    pub fn replay_snapshot(&self) -> Vec<Frame> {
        self.replay.lock().iter().cloned().collect()
    }

    /// Current receiver count — useful for the UI / monitoring.
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }

    /// The configured replay ring cap. Used by callers that want
    /// to pre-allocate buffers of the right size.
    pub fn replay_cap(&self) -> usize {
        self.replay_cap
    }

    /// When this entry was constructed. Monotonic clock — safe to
    /// subtract from `Instant::now()`. Watchdog uses this to give
    /// freshly-spawned sessions a grace period before the idle
    /// timer starts counting (otherwise a slow-booting harness
    /// gets Ctrl-C'd before it finishes printing its first prompt).
    pub fn created_at(&self) -> Instant {
        self.created_at
    }

    /// Timestamp of the most recent `publish()`. Re-computed every
    /// time a frame is broadcast. Watchdog subtracts this from
    /// `now()` to get idle duration.
    pub fn last_frame_at(&self) -> Instant {
        *self.last_frame_at.lock()
    }

    /// How long the session has gone without emitting a frame, as
    /// of `now`. Small convenience around `now - last_frame_at()`
    /// that saturates at zero if the monotonic clock hiccups (can
    /// happen across suspend/resume on macOS).
    pub fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_frame_at())
    }
}

impl Default for SessionEntry {
    fn default() -> Self {
        Self::new()
    }
}
