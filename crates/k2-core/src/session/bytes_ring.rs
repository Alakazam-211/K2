//! BytesRing — in-memory byte replay buffer for the Session byte
//! stream (Canvas Plan Phase 2).
//!
//! Parallel to the Frame replay ring on `SessionEntry`, but stores
//! raw PTY bytes instead of parsed Frames. Subscribers that want
//! pixel-perfect reflow (via a local `alacritty_terminal::Term`)
//! drain the ring first to catch up, then tail the live broadcast.
//!
//! **Authoritative offset tracking.** Every chunk pushed into the
//! ring carries a byte-offset range `[start, end)` where `start` is
//! the cumulative byte count of everything that came before it in
//! this Session's lifetime. `back_offset()` = `total_written` =
//! the offset the next push will land at. `front_offset()` =
//! offset of the earliest byte still in memory (advances when we
//! evict to stay under `cap_bytes`).
//!
//! A subscriber asking for bytes from offset `F`:
//!   - If `F >= back_offset`: nothing to replay, jump straight to
//!     the live broadcast.
//!   - If `F >= front_offset`: the ring has everything they need;
//!     `snapshot_from(F)` returns it.
//!   - If `F < front_offset`: the ring has evicted some of what
//!     they need; the caller must read from the on-disk byte
//!     archive to cover `[F, front_offset)`, then call
//!     `snapshot_from(front_offset)` for the rest.
//!
//! Chunks are `Arc<[u8]>` so broadcasting to N receivers doesn't
//! copy the payload — each recv is a refcount bump.

use std::collections::VecDeque;
use std::sync::Arc;

/// Default in-memory byte ring capacity. 8 MiB is enough to hold
/// many minutes of typical TUI output (Claude's full resume paint
/// is ~tens of KB) while keeping per-session memory bounded. Late
/// attachers for long-running sessions fall through to the on-disk
/// archive for bytes that have aged out.
pub const BYTES_RING_CAP: usize = 8 * 1024 * 1024;

/// One contiguous chunk in the ring. Tagged with the absolute byte
/// offset it starts at so replay is unambiguous under eviction.
#[derive(Debug, Clone)]
pub struct RingChunk {
    /// Absolute byte offset (in the Session's byte stream) of the
    /// first byte in `data`.
    pub start_offset: u64,
    /// Owned-shared byte slice. Arc so broadcasting doesn't clone.
    pub data: Arc<[u8]>,
}

impl RingChunk {
    /// One-past-last absolute byte offset of this chunk.
    pub fn end_offset(&self) -> u64 {
        self.start_offset + self.data.len() as u64
    }
}

/// Bounded byte ring with offset tracking. Not thread-safe on its
/// own — `SessionEntry` wraps it in a `parking_lot::Mutex`.
#[derive(Debug)]
pub struct BytesRing {
    /// Chunks in insertion order. Oldest at front; newest at back.
    chunks: VecDeque<RingChunk>,
    /// Total bytes currently stored across all chunks. Kept in sync
    /// with `chunks` so we can evict in O(1).
    current_bytes: usize,
    /// Soft cap. Push evicts from the front until we're at-or-below
    /// this value (but always keeps at least the just-pushed chunk
    /// even if it alone exceeds the cap — we never drop live data).
    cap_bytes: usize,
    /// Absolute byte offset of the next byte to be written.
    /// Equivalent to "total bytes ever written to this ring." Grows
    /// monotonically; never rewound by eviction.
    total_written: u64,
    /// Absolute byte offset of the first byte currently in the
    /// ring. Equal to `chunks.front().start_offset` when non-empty;
    /// equal to `total_written` when empty (ring drained or never
    /// written). Tracked separately so empty-ring semantics are
    /// explicit.
    front_offset: u64,
}

impl BytesRing {
    /// Construct with the default cap (`BYTES_RING_CAP`).
    pub fn new() -> Self {
        Self::with_cap(BYTES_RING_CAP)
    }

    /// Construct with a custom cap. Must be >= 1 (zero-cap would
    /// discard every push).
    pub fn with_cap(cap_bytes: usize) -> Self {
        assert!(cap_bytes >= 1, "BytesRing cap_bytes must be >= 1");
        Self {
            chunks: VecDeque::new(),
            current_bytes: 0,
            cap_bytes,
            total_written: 0,
            front_offset: 0,
        }
    }

    /// Offset of the first byte currently in memory. `0` initially;
    /// grows as eviction progresses. `== back_offset()` when the
    /// ring is empty.
    pub fn front_offset(&self) -> u64 {
        self.front_offset
    }

    /// Offset of the NEXT byte to be written. Equal to the total
    /// byte count ever pushed to this ring, regardless of eviction.
    pub fn back_offset(&self) -> u64 {
        self.total_written
    }

    /// Current in-memory byte count (sum of chunk sizes).
    pub fn len_bytes(&self) -> usize {
        self.current_bytes
    }

    /// Push a chunk onto the ring. Returns the absolute offset the
    /// chunk starts at (before this push, that offset didn't
    /// exist). Evicts oldest chunks if the ring exceeds `cap_bytes`.
    ///
    /// Pushing a zero-length chunk is a no-op (returns the current
    /// `back_offset` and mutates nothing).
    pub fn push(&mut self, data: Arc<[u8]>) -> u64 {
        let start_offset = self.total_written;
        if data.is_empty() {
            return start_offset;
        }
        let size = data.len();
        self.chunks.push_back(RingChunk {
            start_offset,
            data,
        });
        self.current_bytes += size;
        self.total_written += size as u64;
        self.evict_to_cap();
        start_offset
    }

    /// Evict oldest chunks until `current_bytes <= cap_bytes`.
    /// Always retains at least the newest chunk so a single giant
    /// push doesn't drain the ring to empty (we'd rather blow the
    /// cap slightly than lose live data).
    fn evict_to_cap(&mut self) {
        while self.current_bytes > self.cap_bytes && self.chunks.len() > 1 {
            let evicted = self
                .chunks
                .pop_front()
                .expect("len() > 1, pop_front must succeed");
            self.current_bytes -= evicted.data.len();
            self.front_offset = evicted.end_offset();
        }
    }

    /// Return every chunk whose byte range overlaps `[from, ∞)`.
    /// The first returned chunk may straddle `from` — caller must
    /// skip `from - chunk.start_offset` bytes of its `data`.
    ///
    /// If `from > back_offset()` the result is empty (caller is
    /// asking for future bytes; they should subscribe to the live
    /// broadcast instead).
    ///
    /// If `from < front_offset()` the result covers
    /// `[front_offset, back_offset)` only — the caller needs the
    /// on-disk archive for `[from, front_offset)`.
    pub fn snapshot_from(&self, from: u64) -> Vec<RingChunk> {
        if from >= self.total_written {
            return Vec::new();
        }
        let mut out = Vec::new();
        for chunk in &self.chunks {
            if chunk.end_offset() <= from {
                continue;
            }
            out.push(chunk.clone());
        }
        out
    }

    /// Clone every chunk currently in the ring (oldest → newest).
    /// Convenience for callers that want the full contents without
    /// filtering by offset.
    pub fn snapshot_all(&self) -> Vec<RingChunk> {
        self.chunks.iter().cloned().collect()
    }
}

impl Default for BytesRing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(bytes: &[u8]) -> Arc<[u8]> {
        Arc::from(bytes.to_vec().into_boxed_slice())
    }

    #[test]
    fn empty_ring_invariants() {
        let r = BytesRing::new();
        assert_eq!(r.front_offset(), 0);
        assert_eq!(r.back_offset(), 0);
        assert_eq!(r.len_bytes(), 0);
        assert!(r.snapshot_from(0).is_empty());
        assert!(r.snapshot_all().is_empty());
    }

    #[test]
    fn push_tracks_offsets() {
        let mut r = BytesRing::new();
        assert_eq!(r.push(chunk(b"hello")), 0);
        assert_eq!(r.back_offset(), 5);
        assert_eq!(r.push(chunk(b" world")), 5);
        assert_eq!(r.back_offset(), 11);
        assert_eq!(r.front_offset(), 0);
        assert_eq!(r.len_bytes(), 11);
    }

    #[test]
    fn push_empty_is_noop() {
        let mut r = BytesRing::new();
        r.push(chunk(b"hi"));
        let before = r.back_offset();
        assert_eq!(r.push(chunk(b"")), before);
        assert_eq!(r.back_offset(), before);
    }

    #[test]
    fn eviction_advances_front_offset() {
        let mut r = BytesRing::with_cap(10);
        r.push(chunk(b"aaaa")); // 0..4
        r.push(chunk(b"bbbb")); // 4..8
        r.push(chunk(b"cccc")); // 8..12 → total 12, cap 10, evict front
        assert_eq!(r.front_offset(), 4);
        assert_eq!(r.back_offset(), 12);
        assert_eq!(r.len_bytes(), 8);
        // Another push pushes us over again.
        r.push(chunk(b"dddd")); // 12..16 → total 12, cap 10, evict again
        assert_eq!(r.front_offset(), 8);
        assert_eq!(r.back_offset(), 16);
    }

    #[test]
    fn single_oversize_chunk_is_retained() {
        // A chunk bigger than the cap should stay in the ring as
        // the sole occupant — we never drop live data mid-push.
        let mut r = BytesRing::with_cap(4);
        r.push(chunk(b"aaaaaaaa")); // 8 bytes into a 4-byte ring
        assert_eq!(r.front_offset(), 0);
        assert_eq!(r.back_offset(), 8);
        assert_eq!(r.len_bytes(), 8);
    }

    #[test]
    fn snapshot_from_future_offset_is_empty() {
        let mut r = BytesRing::new();
        r.push(chunk(b"abc"));
        assert!(r.snapshot_from(100).is_empty());
    }

    #[test]
    fn snapshot_from_at_offset_returns_overlapping_chunks() {
        let mut r = BytesRing::new();
        r.push(chunk(b"aaaa")); // 0..4
        r.push(chunk(b"bbbb")); // 4..8
        r.push(chunk(b"cccc")); // 8..12
        // Ask from offset 5 (mid-chunk).
        let got = r.snapshot_from(5);
        // Two chunks: 4..8 (straddles 5) and 8..12.
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].start_offset, 4);
        assert_eq!(got[1].start_offset, 8);
    }

    #[test]
    fn snapshot_from_before_front_returns_available_tail() {
        let mut r = BytesRing::with_cap(6);
        r.push(chunk(b"aaaa")); // 0..4
        r.push(chunk(b"bbbb")); // 4..8 → evicts 0..4; front now 4
        assert_eq!(r.front_offset(), 4);
        // Caller asks from offset 0; we can only serve from 4.
        let got = r.snapshot_from(0);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].start_offset, 4);
        assert_eq!(&*got[0].data, b"bbbb");
    }
}
