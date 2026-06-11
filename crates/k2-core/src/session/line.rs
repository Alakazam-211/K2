//! Line — the unit of the scrollback.
//!
//! The line-oriented scrollback is the core structural difference
//! between K2SO's new producer and a traditional terminal emulator.
//! Each consumer wraps lines at its own viewport width — nothing
//! here carries grid-level layout. See
//! `.k2so/prds/session-stream-and-awareness-bus.md` §"Primitive A".
//!
//! The `SeqnoGen` monotonic stamp pattern mirrors the 0.32.13
//! alacritty_backend seqno damage tracking (`Arc<AtomicU64>` per
//! terminal instance); consumers compare against their last-seen
//! seqno to find dirty lines without re-hashing the buffer.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Monotonic per-line sequence number. `u64` — we'll never exhaust
/// 2^64 lines in a single session lifetime.
pub type SequenceNo = u64;

/// A single logical line of terminal output. Append-only; width-free;
/// owned by the daemon's scrollback buffer. Consumers subscribe to
/// line events and wrap locally at their own viewport width.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Line {
    /// Monotonic stamp bumped on any mutation that changes what the
    /// line looks like. Phase 1 only bumps on creation (append-only
    /// model); Phase 2 may bump on in-place style edits.
    pub seqno: SequenceNo,
    /// UTF-8 text content. No newline terminator.
    pub text: String,
}

impl Line {
    /// Build an empty line with the given seqno.
    pub fn new(seqno: SequenceNo) -> Self {
        Self {
            seqno,
            text: String::new(),
        }
    }

    /// Append text to the line's buffer.
    pub fn push_str(&mut self, s: &str) {
        self.text.push_str(s);
    }
}

/// Monotonic sequence generator. Cheap to clone (wraps an `Arc`).
/// One generator per `LineMux`; multiple consumers of the same
/// session share the same counter through the Arc.
#[derive(Debug, Clone, Default)]
pub struct SeqnoGen {
    inner: Arc<AtomicU64>,
}

impl SeqnoGen {
    /// Bump and return the next seqno. First call returns `1` — we
    /// reserve `0` for the "never seen" sentinel consumers use
    /// before their first comparison.
    pub fn next(&self) -> SequenceNo {
        self.inner.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Read the current seqno without bumping. Used by consumers to
    /// sync their last-seen marker on connect.
    pub fn current(&self) -> SequenceNo {
        self.inner.load(Ordering::Relaxed)
    }
}
