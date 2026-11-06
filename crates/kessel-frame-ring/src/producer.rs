//! The producer: attaches to a ring the consumer made and writes frames.

use crate::layout::*;
use crate::shm::{monotonic_ns, try_lock, unlock, Map};
use crate::{scale, EndReason, Layout, Nv12Frame, RingError};
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
    layout: Layout,
    next_frame: u64,
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
    /// when the file isn't a ring of this version or its length is wrong.
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
        let map = Map::new(&file, len)?;
        let layout = map.read_layout()?;
        map.u64(H_ATTACHES).fetch_add(1, Ordering::Relaxed);
        let latest = map.u64(H_LATEST).load(Ordering::Acquire);
        Ok(Producer {
            map,
            file,
            layout,
            next_frame: after(latest >> 8),
            last_kept_ns: None,
            finished: false,
        })
    }

    /// The ring's layout (its rate as the consumer last set it).
    pub fn layout(&self) -> Layout {
        let mut l = self.layout;
        l.max_fps = self.map.u32(H_MAX_FPS).load(Ordering::Relaxed);
        l
    }

    /// Say the frames are starting (the consumer sees `Running`).
    pub fn start(&self) {
        self.map.u32(H_END_REASON).store(0, Ordering::Relaxed);
        self.map.u32(H_STATE).store(S_RUNNING, Ordering::Release);
    }

    fn flags(&self) -> u32 {
        self.map.u32(H_FLAGS).load(Ordering::Acquire)
    }

    /// Is the consumer there (not gone, and looked within
    /// [`crate::CONSUMER_STALE_NS`])?
    pub fn consumer_present(&self) -> bool {
        let beat = self.map.u64(H_BEAT).load(Ordering::Acquire);
        self.flags() & F_GONE == 0 && monotonic_ns().saturating_sub(beat) < crate::CONSUMER_STALE_NS
    }

    /// Does the consumer want frames now? When false, pushes are dropped
    /// at once; the caller can skip its own work too.
    pub fn wants_frames(&self) -> bool {
        self.flags() & F_WANTS != 0 && self.consumer_present()
    }

    /// Has the consumer asked the producer to stop (the person hung up in
    /// the app)? An iOS broadcast extension then finishes the broadcast.
    pub fn stop_requested(&self) -> bool {
        self.flags() & F_STOP != 0
    }

    /// Write one frame, scaled down to the box if it is larger, into the
    /// slot after the newest, and publish it. Never blocks.
    pub fn push(&mut self, f: &Nv12Frame<'_>) -> Result<Pushed, RingError> {
        f.validate()?;
        if !self.wants_frames() {
            self.map.u64(H_DROPPED).fetch_add(1, Ordering::Relaxed);
            return Ok(Pushed::NotWanted);
        }
        let ts = if f.timestamp_ns == 0 {
            monotonic_ns()
        } else {
            f.timestamp_ns
        };
        let fps = self.map.u32(H_MAX_FPS).load(Ordering::Relaxed);
        if let (Some(last), true) = (self.last_kept_ns, fps > 0) {
            // Keep a frame once 3/4 of the interval has passed, so a source
            // a little faster than the rate isn't halved.
            let interval = 1_000_000_000 / fps as u64;
            if ts.saturating_sub(last) < interval - interval / 4 {
                self.map.u64(H_DROPPED).fetch_add(1, Ordering::Relaxed);
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
        assert!(
            len <= self.layout.slot_bytes(),
            "a fitted frame fits a slot"
        );
        let frame_no = self.next_frame;
        let slot = self.next_slot();
        let base = HEADER + slot * self.layout.slot_stride();
        let seq = self.map.u64(base + SL_SEQ);
        // Odd while writing (a killed writer's slot is odd already).
        let odd = seq.load(Ordering::Relaxed) | 1;
        seq.store(odd, Ordering::Relaxed);
        fence(Ordering::Release);
        self.map
            .u64(base + SL_FRAME_NO)
            .store(frame_no, Ordering::Relaxed);
        self.map.u32(base + SL_WIDTH).store(dw, Ordering::Relaxed);
        self.map.u32(base + SL_HEIGHT).store(dh, Ordering::Relaxed);
        self.map
            .u32(base + SL_ROTATION)
            .store(f.rotation, Ordering::Relaxed);
        self.map
            .u32(base + SL_COLOR)
            .store(f.color as u32, Ordering::Relaxed);
        self.map
            .u64(base + SL_TIMESTAMP)
            .store(ts, Ordering::Relaxed);
        self.map
            .u64(base + SL_LEN)
            .store(len as u64, Ordering::Relaxed);
        let data = self.map.bytes(base + SLOT_HEADER, len);
        // SAFETY: `data` is `len` bytes of this slot (bounds checked by
        // `bytes`), and `len` is the NV12 length of `dw` x `dh`. It is
        // written through the raw pointer, never a `&mut [u8]`, because the
        // consumer may copy the same bytes at the same time (a seqlock: it
        // checks the slot's sequence after and drops a copy that raced this
        // write).
        unsafe { scale::write_nv12(f, dw, dh, data) };
        seq.store(odd + 1, Ordering::Release);
        self.map
            .u64(H_LATEST)
            .store(frame_no << 8 | slot as u64, Ordering::Release);
        self.map.u64(H_WRITTEN).fetch_add(1, Ordering::Relaxed);
        self.next_frame = after(frame_no);
        self.last_kept_ns = Some(ts);
        Ok(Pushed::Written {
            frame_no,
            width: dw,
            height: dh,
        })
    }

    /// The slot after the newest published one (never the newest itself,
    /// which a reader may be copying).
    fn next_slot(&self) -> usize {
        let slots = self.layout.slots as usize;
        let latest = self.map.u64(H_LATEST).load(Ordering::Acquire);
        if latest == 0 {
            return 0;
        }
        ((latest & 0xff) as usize + 1) % slots
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
        self.map
            .u32(H_END_REASON)
            .store(reason as u32, Ordering::Relaxed);
        self.map.u32(H_STATE).store(S_ENDED, Ordering::Release);
    }

    /// Start writing a frame and stop halfway, as a writer killed
    /// mid-frame would: the slot is left odd and unpublished.
    #[cfg(test)]
    pub(crate) fn push_torn(&mut self, value: u8) {
        let slot = self.next_slot();
        let base = HEADER + slot * self.layout.slot_stride();
        let seq = self.map.u64(base + SL_SEQ);
        seq.store(seq.load(Ordering::Relaxed) | 1, Ordering::Relaxed);
        fence(Ordering::Release);
        self.map
            .u64(base + SL_FRAME_NO)
            .store(self.next_frame, Ordering::Relaxed);
        let half = self.layout.slot_bytes() / 2;
        let data = self.map.bytes(base + SLOT_HEADER, half);
        // SAFETY: `half` bytes of this slot, bounds checked by `bytes`.
        unsafe { std::ptr::write_bytes(data, value, half) };
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

impl Drop for Producer {
    fn drop(&mut self) {
        // Dropped without a reason: the producer is closing.
        self.end(EndReason::SourceClosed);
        unlock(&self.file);
    }
}
