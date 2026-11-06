//! # kessel-frame-ring
//!
//! A single-producer, single-consumer ring of NV12 video frames in a
//! shared-memory file, for handing frames from one process to another on
//! the same machine. It was made for an iPhone's screen share: the broadcast
//! upload extension (the producer, its own process with a 50 MB cap) copies
//! each screen frame in, and the app (the consumer) reads it out and
//! encodes it.
//!
//! - **The consumer creates the ring** ([`Consumer::create`]) in a file both
//!   processes can open (an App Group container on iOS), with a fixed
//!   [`Layout`]: the largest frame (either way round), the slot count and the
//!   highest rate. The producer maps it read-write ([`Producer::attach`]).
//! - **The producer never blocks and never waits for the consumer.** It
//!   scales each frame down to fit the layout's box, writes it into the slot
//!   after the newest one, and publishes it. When the consumer doesn't want
//!   frames, is gone, or hasn't looked for two seconds (the app suspended),
//!   pushes return at once without copying anything.
//! - **Each frame carries its own size, rotation, colour and timestamp**, so
//!   a turn or a new size needs no re-layout. Only the capacity is fixed.
//! - **Torn frames are never read.** Each slot has a sequence word (a
//!   seqlock): odd while being written, even when whole. A producer killed
//!   mid-write leaves its slot odd and unpublished; the next producer
//!   rewrites it. A consumer suspended mid-read finds the sequence moved and
//!   drops the copy.
//! - **The control words** carry the producer's state (waiting, running,
//!   ended and why) and the consumer's wishes (wants frames, please stop,
//!   gone, highest rate). A producer holds an advisory lock on the file
//!   while attached, so the consumer can tell a killed producer
//!   ([`ProducerState::Gone`]) from one that is merely quiet.
//! - **Memory stays bounded:** the file is the header and the slots, sized
//!   once; nothing is allocated per frame on the producer's side.
//! - **The consumer trusts nothing in the file.** The producer is another
//!   process, so a frame's slot, size, rotation, colour and length are
//!   checked against the consumer's own layout before a byte is copied
//!   (see [`FrameInfo`]), and the file's length before each use of the
//!   mapping. The README's "Trust boundary" says what that covers and
//!   what it can't.
//!
//! The producer's whole surface is also a small `extern "C"` API ([`ffi`]),
//! for an app extension written in Swift that links this crate as a static
//! library. Unix only (macOS, iOS, Linux, Android).
//!
//! ```
//! # #[cfg(unix)] {
//! use kessel_frame_ring::{Consumer, Layout, Nv12Frame, Producer, Pushed};
//!
//! let path = std::env::temp_dir().join(format!("doc-ring-{}", std::process::id()));
//! let mut consumer = Consumer::create(&path, Layout::new(64, 48))?;
//! let mut producer = Producer::attach(&path)?;
//! producer.start();
//! let (y, uv) = (vec![100u8; 64 * 48], vec![128u8; 64 * 24]);
//! let frame = Nv12Frame::new(&y, 64, &uv, 64, 64, 48);
//! assert!(matches!(producer.push(&frame)?, Pushed::Written { .. }));
//! let mut buf = Vec::new();
//! let info = consumer.try_read(&mut buf).expect("a frame");
//! assert_eq!((info.width, info.height, buf.len()), (64, 48, 64 * 48 * 3 / 2));
//! # }
//! # Ok::<(), kessel_frame_ring::RingError>(())
//! ```

// Off Unix only the types build; the layout's words go unused.
#![cfg_attr(not(unix), allow(dead_code))]

#[cfg(unix)]
mod consumer;
#[cfg(unix)]
pub mod ffi;
#[cfg(unix)]
mod producer;
#[cfg(unix)]
mod scale;
#[cfg(unix)]
mod shm;
#[cfg(all(test, unix))]
mod hostile_tests;
#[cfg(all(test, unix))]
mod tests;

#[cfg(unix)]
pub use consumer::Consumer;
#[cfg(unix)]
pub use producer::{Producer, Pushed};
#[cfg(unix)]
pub use shm::monotonic_ns;

/// The format version in the header. A file of another version is refused.
pub const VERSION: u32 = 1;
/// The most slots a ring may have.
pub const MAX_SLOTS: u32 = 8;
/// The largest side of the layout's box.
pub const MAX_SIDE: u32 = 4096;
/// The largest side of a frame handed to [`Producer::push`] (it is scaled
/// down to the box).
pub const MAX_INPUT_SIDE: u32 = 16_384;
/// The consumer counts as away when it hasn't looked for this long (ns).
pub const CONSUMER_STALE_NS: u64 = 2_000_000_000;

/// The ring's fixed shape, set by the consumer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Layout {
    /// The widest frame, landscape (even, 16 to [`MAX_SIDE`]). The box is
    /// orientation-free: a 1280 x 720 layout also takes 720 x 1280.
    pub max_width: u32,
    /// The tallest frame, landscape (even, 16 to [`MAX_SIDE`]).
    pub max_height: u32,
    /// Slots (2 to [`MAX_SLOTS`]): the newest frame, one being written, and
    /// one spare for a slow reader. Default 3.
    pub slots: u32,
    /// The producer drops frames that come faster than this (0: no limit).
    /// The consumer can change it while running. Default 15.
    pub max_fps: u32,
}

impl Layout {
    /// A `max_width` x `max_height` box, 3 slots, at most 15 fps.
    pub fn new(max_width: u32, max_height: u32) -> Layout {
        Layout {
            max_width,
            max_height,
            slots: 3,
            max_fps: 15,
        }
    }

    /// 1280 x 720 (either way round), 3 slots, 15 fps: about 4 MB.
    pub fn hd720() -> Layout {
        Layout::new(1280, 720)
    }

    /// Check the fields.
    pub fn validate(&self) -> Result<(), RingError> {
        for (what, v) in [("width", self.max_width), ("height", self.max_height)] {
            if !(16..=MAX_SIDE).contains(&v) || !v.is_multiple_of(2) {
                return Err(RingError::Invalid(format!("a max {what} of {v}")));
            }
        }
        if !(2..=MAX_SLOTS).contains(&self.slots) {
            return Err(RingError::Invalid(format!("{} slots", self.slots)));
        }
        if self.max_fps > 240 {
            return Err(RingError::Invalid(format!("{} fps", self.max_fps)));
        }
        Ok(())
    }

    /// The bytes one slot holds: an NV12 frame of the box.
    pub fn slot_bytes(&self) -> usize {
        nv12_len(self.max_width, self.max_height)
    }

    /// The whole file's length (`usize::MAX` for a layout too large to
    /// exist, which [`Layout::validate`] refuses).
    pub fn file_len(&self) -> usize {
        (self.slots as usize)
            .saturating_mul(self.slot_stride())
            .saturating_add(layout::HEADER)
    }

    pub(crate) fn slot_stride(&self) -> usize {
        layout::SLOT_HEADER
            .saturating_add(self.slot_bytes())
            .div_ceil(64)
            .saturating_mul(64)
    }

    /// Does a `width` x `height` frame fit the box, either way round?
    pub(crate) fn fits(&self, width: u32, height: u32) -> bool {
        let (bw, bh) = (self.max_width, self.max_height);
        (width <= bw && height <= bh) || (width <= bh && height <= bw)
    }
}

/// The bytes of a tight NV12 frame: the Y plane, then interleaved U and V
/// at half size (rounded up). `usize::MAX` when that doesn't fit a `usize`
/// (no real frame is that large), so it never overflows.
pub fn nv12_len(width: u32, height: u32) -> usize {
    let (w, h) = (u64::from(width), u64::from(height));
    // Each term fits a u64 (under 2^64 and 2^63); only the sum may not.
    let len = (w * h).saturating_add(2 * w.div_ceil(2) * h.div_ceil(2));
    usize::try_from(len).unwrap_or(usize::MAX)
}

/// The colour description of a frame's YUV.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u32)]
#[non_exhaustive]
pub enum Color {
    /// BT.601, limited range (16-235).
    #[default]
    Bt601Limited = 0,
    /// BT.709, limited range (`420v` pixel buffers).
    Bt709Limited = 1,
    /// BT.709, full range (`420f` pixel buffers).
    Bt709Full = 2,
}

impl Color {
    /// From its number (0, 1 or 2).
    pub fn from_u32(v: u32) -> Option<Color> {
        match v {
            0 => Some(Color::Bt601Limited),
            1 => Some(Color::Bt709Limited),
            2 => Some(Color::Bt709Full),
            _ => None,
        }
    }
}

/// Why the producer stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
#[non_exhaustive]
pub enum EndReason {
    /// The person stopped it (an iOS broadcast's `broadcastFinished`).
    UserStopped = 1,
    /// The producer went away for another reason, or was dropped without
    /// saying why.
    SourceClosed = 2,
    /// The producer failed.
    Failed = 3,
}

impl EndReason {
    /// From its number (1, 2 or 3).
    pub fn from_u32(v: u32) -> Option<EndReason> {
        match v {
            1 => Some(EndReason::UserStopped),
            2 => Some(EndReason::SourceClosed),
            3 => Some(EndReason::Failed),
            _ => None,
        }
    }
}

/// What the consumer knows of the producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProducerState {
    /// None attached yet, or attached and not started.
    Waiting,
    /// Started and alive.
    Running,
    /// Stopped, and said why.
    Ended(EndReason),
    /// Started, then died without a word (killed, or crashed): its lock on
    /// the file is gone.
    Gone,
}

/// One frame for [`Producer::push`]: NV12 planes with any row strides
/// (bytes per row of each plane), as a `CVPixelBuffer` hands them out.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Nv12Frame<'a> {
    /// The Y plane: `height` rows of `y_stride` bytes (the last row may
    /// stop at `width`).
    pub y: &'a [u8],
    /// Bytes per Y row (at least `width`).
    pub y_stride: usize,
    /// The interleaved U and V plane: half the rows (rounded up).
    pub uv: &'a [u8],
    /// Bytes per UV row (at least `width` rounded up to even).
    pub uv_stride: usize,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How far the frame must turn clockwise to stand upright: 0, 90, 180
    /// or 270. It travels with the frame; the consumer turns it.
    pub rotation: u32,
    /// The colour description.
    pub color: Color,
    /// When it was captured, in ns on [`monotonic_ns`]'s clock (the same
    /// in every process: mach absolute time on Apple, `CLOCK_MONOTONIC`
    /// elsewhere). 0: stamp it now.
    pub timestamp_ns: u64,
}

impl<'a> Nv12Frame<'a> {
    /// Upright, BT.601 limited range, stamped when pushed.
    pub fn new(
        y: &'a [u8],
        y_stride: usize,
        uv: &'a [u8],
        uv_stride: usize,
        width: u32,
        height: u32,
    ) -> Nv12Frame<'a> {
        Nv12Frame {
            y,
            y_stride,
            uv,
            uv_stride,
            width,
            height,
            rotation: 0,
            color: Color::Bt601Limited,
            timestamp_ns: 0,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), RingError> {
        let bad = |why: String| Err(RingError::Invalid(why));
        let (w, h) = (self.width as usize, self.height as usize);
        if !(1..=MAX_INPUT_SIDE).contains(&self.width)
            || !(1..=MAX_INPUT_SIDE).contains(&self.height)
        {
            return bad(format!("a {w}x{h} frame"));
        }
        if !matches!(self.rotation, 0 | 90 | 180 | 270) {
            return bad(format!("a rotation of {}", self.rotation));
        }
        // A plane of `rows` rows `stride` apart, the last `row` bytes long,
        // needs this many bytes; None when that overflows (a stride no real
        // plane has).
        let need =
            |stride: usize, rows: usize, row: usize| stride.checked_mul(rows - 1)?.checked_add(row);
        if self.y_stride < w {
            return bad(format!("a Y stride of {} for {w} pixels", self.y_stride));
        }
        if need(self.y_stride, h, w).is_none_or(|n| self.y.len() < n) {
            return bad(format!(
                "a {}-byte Y plane for {w}x{h}, stride {}",
                self.y.len(),
                self.y_stride
            ));
        }
        let (cw2, ch) = (2 * w.div_ceil(2), h.div_ceil(2));
        if self.uv_stride < cw2 {
            return bad(format!("a UV stride of {} for {w} pixels", self.uv_stride));
        }
        if need(self.uv_stride, ch, cw2).is_none_or(|n| self.uv.len() < n) {
            return bad(format!(
                "a {}-byte UV plane for {w}x{h}, stride {}",
                self.uv.len(),
                self.uv_stride
            ));
        }
        Ok(())
    }
}

/// One frame read by [`Consumer::try_read`]: what it is, and where its
/// planes sit in the bytes read out (tight NV12).
///
/// The producer is another process, so the consumer checks what it wrote
/// before handing a frame out: the width and height are at least 1 and fit
/// the ring's box (either way round), the rotation is 0, 90, 180 or 270,
/// the colour is one this version knows, and the bytes read out are exactly
/// [`FrameInfo::len`]. A slot that fails a check is skipped
/// ([`RingStats::rejected`]). The timestamp and the frame number are only
/// numbers: use them with saturating or checked maths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct FrameInfo {
    /// Counts up from 1 across producers (to 2^56 - 1, then from 1 again);
    /// a gap means frames were skipped.
    pub frame_no: u64,
    /// Width in pixels (after the producer's scaling).
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// How far to turn it clockwise to stand upright: 0, 90, 180 or 270.
    pub rotation: u32,
    /// The colour description.
    pub color: Color,
    /// Capture time, ns on [`monotonic_ns`]'s clock, as the producer
    /// stamped it.
    pub timestamp_ns: u64,
}

impl FrameInfo {
    /// Bytes per Y row (the width).
    pub fn y_stride(&self) -> usize {
        self.width as usize
    }

    /// Where the UV plane starts.
    pub fn uv_offset(&self) -> usize {
        (self.width as usize).saturating_mul(self.height as usize)
    }

    /// Bytes per UV row (the width rounded up to even).
    pub fn uv_stride(&self) -> usize {
        2 * (self.width as usize).div_ceil(2)
    }

    /// The whole frame's bytes.
    pub fn len(&self) -> usize {
        nv12_len(self.width, self.height)
    }

    /// Never: a frame has at least one pixel.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// The consumer's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RingStats {
    /// Frames the producers wrote.
    pub written: u64,
    /// Frames the producers dropped (not wanted, or over the rate).
    pub dropped: u64,
    /// Frames written but never read (the consumer read a newer one).
    pub missed: u64,
    /// Reads given up because the producer kept rewriting the slot.
    pub torn: u64,
    /// Frames skipped because their slot didn't describe a valid frame (a
    /// corrupt ring, or a producer that misbehaves). Counted once per
    /// frame.
    pub rejected: u64,
    /// Producers that attached.
    pub attaches: u64,
}

/// What went wrong.
#[derive(Debug)]
#[non_exhaustive]
pub enum RingError {
    /// No ring at that path: the consumer hasn't made one (on iOS, the app
    /// isn't in a call).
    NotFound,
    /// Another producer is attached.
    Busy,
    /// The file isn't a ring of this version, or its length doesn't match
    /// its layout.
    Mismatch(String),
    /// A bad argument: a layout, a frame's size, strides or rotation.
    Invalid(String),
    /// The OS failed.
    Io(std::io::Error),
}

impl RingError {
    /// The `extern "C"` code: -1 invalid, -2 not found, -3 busy, -4
    /// mismatch, -5 I/O.
    pub fn code(&self) -> i32 {
        match self {
            RingError::Invalid(_) => -1,
            RingError::NotFound => -2,
            RingError::Busy => -3,
            RingError::Mismatch(_) => -4,
            RingError::Io(_) => -5,
        }
    }
}

impl std::fmt::Display for RingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RingError::NotFound => f.write_str("no frame ring there"),
            RingError::Busy => f.write_str("another producer is attached"),
            RingError::Mismatch(why) => write!(f, "not a matching frame ring: {why}"),
            RingError::Invalid(why) => write!(f, "invalid: {why}"),
            RingError::Io(e) => write!(f, "I/O: {e}"),
        }
    }
}

impl std::error::Error for RingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RingError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for RingError {
    fn from(e: std::io::Error) -> RingError {
        if e.kind() == std::io::ErrorKind::NotFound {
            RingError::NotFound
        } else {
            RingError::Io(e)
        }
    }
}

/// The file's layout: a 256-byte header, then the slots, each a 64-byte
/// header and an NV12 frame of the box, rounded up to 64 bytes. Every
/// number is in the machine's byte order (both sides run on one machine).
pub(crate) mod layout {
    /// "KFRAMRNG" read as a little-endian u64.
    pub(crate) const MAGIC: u64 = u64::from_le_bytes(*b"KFRAMRNG");
    pub(crate) const HEADER: usize = 256;
    pub(crate) const SLOT_HEADER: usize = 64;

    // Header words. Fixed at create: magic to slot bytes.
    pub(crate) const H_MAGIC: usize = 0; // u64
    pub(crate) const H_VERSION: usize = 8; // u32
    pub(crate) const H_HEADER_SIZE: usize = 12; // u32
    pub(crate) const H_SLOT_HEADER: usize = 16; // u32
    pub(crate) const H_SLOTS: usize = 20; // u32
    pub(crate) const H_MAX_WIDTH: usize = 24; // u32
    pub(crate) const H_MAX_HEIGHT: usize = 28; // u32
    pub(crate) const H_SLOT_BYTES: usize = 32; // u64
                                               // The consumer's words.
    pub(crate) const H_MAX_FPS: usize = 40; // u32
    pub(crate) const H_FLAGS: usize = 44; // u32, F_*
    pub(crate) const H_BEAT: usize = 48; // u64, ns: when the consumer last looked
                                         // The producer's words.
    pub(crate) const H_STATE: usize = 56; // u32, S_*
    pub(crate) const H_END_REASON: usize = 60; // u32, EndReason
    pub(crate) const H_LATEST: usize = 64; // u64: frame_no << 8 | slot; 0 = none
    /// The largest frame number (56 bits, beside the slot in `H_LATEST`).
    /// After it the producer starts again at 1.
    pub(crate) const MAX_FRAME_NO: u64 = (1 << 56) - 1;
    pub(crate) const H_WRITTEN: usize = 72; // u64
    pub(crate) const H_DROPPED: usize = 80; // u64
    pub(crate) const H_ATTACHES: usize = 88; // u64

    /// The consumer wants frames.
    pub(crate) const F_WANTS: u32 = 1;
    /// The consumer asks the producer to stop.
    pub(crate) const F_STOP: u32 = 2;
    /// The consumer is gone.
    pub(crate) const F_GONE: u32 = 4;

    pub(crate) const S_WAITING: u32 = 0;
    pub(crate) const S_RUNNING: u32 = 1;
    pub(crate) const S_ENDED: u32 = 2;

    // Slot header words. Written by the producer under the slot's seqlock.
    pub(crate) const SL_SEQ: usize = 0; // u64: odd while writing
    pub(crate) const SL_FRAME_NO: usize = 8; // u64
    pub(crate) const SL_WIDTH: usize = 16; // u32
    pub(crate) const SL_HEIGHT: usize = 20; // u32
    pub(crate) const SL_ROTATION: usize = 24; // u32
    pub(crate) const SL_COLOR: usize = 28; // u32
    pub(crate) const SL_TIMESTAMP: usize = 32; // u64
    pub(crate) const SL_LEN: usize = 40; // u64
}
