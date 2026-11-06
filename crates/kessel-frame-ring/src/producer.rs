//! The producer: attaches to a ring the consumer made and writes frames.
//!
//! The consumer is another process with the file open read-write, so
//! nothing in the file is trusted here either. The layout is read once, at
//! attach, and checked; after that the producer writes by its own copy.
//! It keeps its own count of each slot's sequence word and of the newest
//! slot, so no sum is ever taken of a word read back from the file: a
//! sequence word it didn't write is overwritten with its own. The
//! consumer's words (the flags, the heartbeat, the rate) are read as plain
//! hints, the rate held to the range a layout allows. The file's length is
//! checked before each use of the mapping (the consumer can shrink the
//! file, and touching a mapped page past its end faults).

use crate::layout::*;
use crate::shm::{monotonic_ns, try_lock, unlock, Map};
use crate::{scale, EndReason, Layout, Nv12Frame, RingError, MAX_SLOTS};
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::atomic::{fence, Ordering};
use std::time::Duration;

/// What [`Producer::push`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Pushed {
    /// Written and published.
    Written {
        /// Its number.
        frame_no: u64,
        /// Its width in the ring (after scaling).
        width: u32,
        /// Its height in the ring.
        height: u32,
    },
    /// Dropped without copying: the consumer doesn't want frames now, is
    /// gone, or hasn't looked for [`crate::CONSUMER_STALE_NS`] (suspended).
    NotWanted,
    /// Dropped: it came sooner than the layout's rate allows.
    TooSoon,
}

/// The writing side of a ring. One per ring at a time.
pub struct Producer {
    map: Map,
    file: File,
    /// The layout as checked at attach. The file's copy is never read
    /// again (only its rate, which the consumer may change).
    layout: Layout,
    next_frame: u64,
    /// Each slot's sequence word as this producer last wrote it, or found
    /// it at attach. The file's own words are never added to.
    seqs: [u64; MAX_SLOTS as usize],
    /// The slot this producer last published, or found published at
    /// attach: below the layout's slot count.
    newest: Option<usize>,
    last_kept_ns: Option<u64>,
    finished: bool,
}

impl std::fmt::Debug for Producer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Producer")
            .field("layout", &self.layout)
            .field("next_frame", &self.next_frame)
            .finish_non_exhaustive()
    }
}

impl Producer {
    /// Open the ring at `path` read-write, check it, and take the
    /// producer's lock. `NotFound` when there is no ring (the consumer
    /// hasn't made one), `Busy` when another producer holds it, `Mismatch`
    /// when the file isn't a ring of this version, its layout is out of
    /// range, or its length is wrong.
    pub fn attach(path: impl AsRef<Path>) -> Result<Producer, RingError> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        // The consumer probes the lock for a moment now and then.
        let mut locked = false;
        for _ in 0..5 {
            if try_lock(&file)? {
                locked = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if !locked {
            return Err(RingError::Busy);
        }
        let len = file.metadata()?.len();
        let len = usize::try_from(len).map_err(|_| RingError::Mismatch(format!("{len} bytes")))?;
        if len < HEADER {
            return Err(RingError::Mismatch(format!(
                "{len} bytes, under the header"
            )));
        }
        // No valid layout makes a larger file: don't map one that can't be
        // a ring.
        if len > largest_file() {
            return Err(RingError::Mismatch(format!(
                "{len} bytes, over the largest ring"
            )));
        }
        let map = Map::new(&file, len)?;
        // Shrunk between the length and the mapping: reading the header
        // could fault. (This narrows the window; see the README.)
        if !whole(&file, len) {
            return Err(shrunk());
        }
        let layout = map.read_layout()?;
        // A plain counter: it wraps, by design.
        map.u64(H_ATTACHES).fetch_add(1, Ordering::Relaxed);
        // Where the last producer left off, each word checked: the frame
        // number through `after`, the slot against the layout's count, and
        // each sequence word against MAX_SEQ.
        let latest = map.u64(H_LATEST).load(Ordering::Acquire);
        let slots = layout.slots as usize;
        let newest = Some((latest & 0xff) as usize).filter(|&s| latest != 0 && s < slots);
        let mut seqs = [0; MAX_SLOTS as usize];
        for (slot, seq) in seqs.iter_mut().enumerate().take(slots) {
            let base = HEADER + slot * layout.slot_stride();
            *seq = sane_seq(map.u64(base + SL_SEQ).load(Ordering::Relaxed));
        }
        Ok(Producer {
            map,
            file,
            layout,
            next_frame: after(latest >> 8),
            seqs,
            newest,
            last_kept_ns: None,
            finished: false,
        })
    }

    /// The mapping, if the file still covers all of it. The consumer can
    /// shrink the file; touching a page past its end would kill this
    /// process (SIGBUS), so every use of the mapping after attach goes
    /// through here. This narrows that window to the time between the
    /// check and the access; it can't close it (see the README).
    fn map(&self) -> Option<&Map> {
        whole(&self.file, self.layout.file_len()).then_some(&self.map)
    }

    /// The ring's layout, with its rate as the consumer last set it (held
    /// to 0 to 240; the rate at attach once the file has shrunk).
    pub fn layout(&self) -> Layout {
        let mut l = self.layout;
        if let Some(map) = self.map() {
            l.max_fps = rate(map);
        }
        l
    }

    /// Say the frames are starting (the consumer sees `Running`). Does
    /// nothing once the file has shrunk.
    pub fn start(&self) {
        if let Some(map) = self.map() {
            map.u32(H_END_REASON).store(0, Ordering::Relaxed);
            map.u32(H_STATE).store(S_RUNNING, Ordering::Release);
        }
    }

    /// Is the consumer there (not gone, and looked within
    /// [`crate::CONSUMER_STALE_NS`])? False once the file has shrunk.
    pub fn consumer_present(&self) -> bool {
        self.map().is_some_and(present)
    }

    /// Does the consumer want frames now? When false, pushes are dropped
    /// at once; the caller can skip its own work too. False once the file
    /// has shrunk.
    pub fn wants_frames(&self) -> bool {
        self.map().is_some_and(wanted)
    }

    /// Has the consumer asked the producer to stop (the person hung up in
    /// the app)? An iOS broadcast extension then finishes the broadcast.
    /// False once the file has shrunk ([`Producer::push`] says so then).
    pub fn stop_requested(&self) -> bool {
        self.map().is_some_and(|m| flags(m) & F_STOP != 0)
    }

    /// Write one frame, scaled down to the box if it is larger, into the
    /// slot after the newest, and publish it. Never blocks. `Invalid` for a
    /// frame that doesn't describe itself; `Mismatch` when the ring's file
    /// has shrunk under it (end the broadcast).
    pub fn push(&mut self, f: &Nv12Frame<'_>) -> Result<Pushed, RingError> {
        f.validate()?;
        // One check for the whole push.
        if self.map().is_none() {
            return Err(shrunk());
        }
        let map = &self.map;
        if !wanted(map) {
            // A plain counter: it wraps, by design.
            map.u64(H_DROPPED).fetch_add(1, Ordering::Relaxed);
            return Ok(Pushed::NotWanted);
        }
        let ts = if f.timestamp_ns == 0 {
            monotonic_ns()
        } else {
            f.timestamp_ns
        };
        let fps = rate(map);
        if let (Some(last), true) = (self.last_kept_ns, fps > 0) {
            // Keep a frame once 3/4 of the interval has passed, so a source
            // a little faster than the rate isn't halved. `fps` is 1 to
            // MAX_FPS here.
            let interval = 1_000_000_000 / u64::from(fps);
            if ts.saturating_sub(last) < interval - interval / 4 {
                map.u64(H_DROPPED).fetch_add(1, Ordering::Relaxed);
                return Ok(Pushed::TooSoon);
            }
        }
        let (dw, dh) = scale::fit(
            f.width,
            f.height,
            self.layout.max_width,
            self.layout.max_height,
        );
        let len = crate::nv12_len(dw, dh);
        if len > self.layout.slot_bytes() {
            // Never: `fit` keeps a frame inside the box. Refused rather
            // than written past the slot.
            return Err(RingError::Mismatch(format!(
                "a {dw}x{dh} frame over a slot of {} bytes",
                self.layout.slot_bytes()
            )));
        }
        let frame_no = self.next_frame;
        let slot = self.next_slot();
        // In the mapping: the slot is below the layout's count, and the
        // layout matched the mapping's length at attach.
        let base = HEADER + slot * self.layout.slot_stride();
        let seq = map.u64(base + SL_SEQ);
        // The producer is the slot's only writer, so its own count is the
        // truth: whatever the file holds now (a killed writer's odd word,
        // or anything else) is overwritten, never added to. Odd while
        // writing. `sane_seq` keeps it at most MAX_SEQ, so `odd + 1` can't
        // overflow.
        let odd = sane_seq(self.seqs[slot]) | 1;
        seq.store(odd, Ordering::Relaxed);
        fence(Ordering::Release);
        map.u64(base + SL_FRAME_NO)
            .store(frame_no, Ordering::Relaxed);
        map.u32(base + SL_WIDTH).store(dw, Ordering::Relaxed);
        map.u32(base + SL_HEIGHT).store(dh, Ordering::Relaxed);
        map.u32(base + SL_ROTATION)
            .store(f.rotation, Ordering::Relaxed);
        map.u32(base + SL_COLOR)
            .store(f.color as u32, Ordering::Relaxed);
        map.u64(base + SL_TIMESTAMP).store(ts, Ordering::Relaxed);
        map.u64(base + SL_LEN).store(len as u64, Ordering::Relaxed);
        let data = map.bytes(base + SLOT_HEADER, len);
        // SAFETY: `data` is `len` bytes of this slot (bounds checked by
        // `bytes`), and `len` is the NV12 length of `dw` x `dh`. It is
        // written through the raw pointer, never a `&mut [u8]`, because the
        // consumer may copy the same bytes at the same time (a seqlock: it
        // checks the slot's sequence after and drops a copy that raced this
        // write).
        unsafe { scale::write_nv12(f, dw, dh, data) };
        let even = odd + 1;
        seq.store(even, Ordering::Release);
        // `frame_no` is 1 to MAX_FRAME_NO (from `after`), so the shift
        // loses no bits.
        map.u64(H_LATEST)
            .store(frame_no << 8 | slot as u64, Ordering::Release);
        map.u64(H_WRITTEN).fetch_add(1, Ordering::Relaxed);
        self.seqs[slot] = even;
        self.newest = Some(slot);
        self.next_frame = after(frame_no);
        self.last_kept_ns = Some(ts);
        Ok(Pushed::Written {
            frame_no,
            width: dw,
            height: dh,
        })
    }

    /// The slot after the newest published one (never the newest itself,
    /// which a reader may be copying), from the producer's own count.
    fn next_slot(&self) -> usize {
        self.newest
            .map_or(0, |s| (s + 1) % self.layout.slots as usize)
    }

    /// Stop for good, saying why: the consumer sees `Ended(reason)`. The
    /// lock is released and the mapping closed.
    pub fn finish(mut self, reason: EndReason) {
        self.end(reason);
    }

    fn end(&mut self, reason: EndReason) {
        if self.finished {
            return;
        }
        self.finished = true;
        if let Some(map) = self.map() {
            map.u32(H_END_REASON)
                .store(reason as u32, Ordering::Relaxed);
            map.u32(H_STATE).store(S_ENDED, Ordering::Release);
        }
    }

    /// Start writing a frame and stop halfway, as a writer killed
    /// mid-frame would: the slot is left odd and unpublished.
    #[cfg(test)]
    pub(crate) fn push_torn(&mut self, value: u8) {
        let slot = self.next_slot();
        let base = HEADER + slot * self.layout.slot_stride();
        let odd = sane_seq(self.seqs[slot]) | 1;
        self.map.u64(base + SL_SEQ).store(odd, Ordering::Relaxed);
        self.seqs[slot] = odd;
        fence(Ordering::Release);
        self.map
            .u64(base + SL_FRAME_NO)
            .store(self.next_frame, Ordering::Relaxed);
        let half = self.layout.slot_bytes() / 2;
        let data = self.map.bytes(base + SLOT_HEADER, half);
        // SAFETY: `half` bytes of this slot, bounds checked by `bytes`.
        unsafe { std::ptr::write_bytes(data, value, half) };
    }

    /// This producer's own count for `slot`'s sequence word.
    #[cfg(test)]
    pub(crate) fn own_seq(&self, slot: usize) -> u64 {
        self.seqs[slot]
    }

    /// The number the next written frame gets.
    #[cfg(test)]
    pub(crate) fn next_frame_no(&self) -> u64 {
        self.next_frame
    }
}

/// The frame number after `n`: 1 after the largest (and after anything
/// out of range a corrupt file held), so `frame_no << 8` never loses bits
/// and 0 (no frame) is never used.
fn after(n: u64) -> u64 {
    if n < MAX_FRAME_NO {
        n + 1
    } else {
        1
    }
}

/// A sequence word the producer may count on from: 0 in place of anything
/// past [`MAX_SEQ`] (corruption, or a count that ran out after 2^61
/// frames), so adding 2 never overflows.
fn sane_seq(v: u64) -> u64 {
    if v > MAX_SEQ {
        0
    } else {
        v
    }
}

/// The consumer's flags.
fn flags(map: &Map) -> u32 {
    map.u32(H_FLAGS).load(Ordering::Acquire)
}

/// The consumer is there: not gone, and looked lately. A heartbeat from
/// the future counts as now.
fn present(map: &Map) -> bool {
    let beat = map.u64(H_BEAT).load(Ordering::Acquire);
    flags(map) & F_GONE == 0 && monotonic_ns().saturating_sub(beat) < crate::CONSUMER_STALE_NS
}

/// The consumer is there and wants frames.
fn wanted(map: &Map) -> bool {
    flags(map) & F_WANTS != 0 && present(map)
}

/// The consumer's rate, held to what a layout allows (0 to MAX_FPS).
fn rate(map: &Map) -> u32 {
    map.u32(H_MAX_FPS).load(Ordering::Relaxed).min(MAX_FPS)
}

/// Does `file` still hold `len` bytes?
fn whole(file: &File, len: usize) -> bool {
    file.metadata().is_ok_and(|m| m.len() >= len as u64)
}

/// The error for a file that has shrunk under the ring.
fn shrunk() -> RingError {
    RingError::Mismatch("the file has shrunk under the ring".into())
}

/// The largest file a valid layout makes.
fn largest_file() -> usize {
    Layout {
        max_width: crate::MAX_SIDE,
        max_height: crate::MAX_SIDE,
        slots: MAX_SLOTS,
        max_fps: 0,
    }
    .file_len()
}

impl Drop for Producer {
    fn drop(&mut self) {
        // Dropped without a reason: the producer is closing.
        self.end(EndReason::SourceClosed);
        unlock(&self.file);
    }
}
