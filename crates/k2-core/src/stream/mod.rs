//! The device stream engine, source side (fix 10, S2). Source-neutral: the
//! Mobile tab's simulators and the remote desktop (k2-deskd) both feed it.
//!
//! - [`SourceKey`]: which source a hub serves (`Sim(id)` or a desktop
//!   display); [`SourceKind`] keys the rate table.
//! - [`EncodedSource`]: what a hub pulls. One encode per **layer**, never per
//!   viewer: the hub sends the whole layer list ([`LayerSpec`], protocol 4's
//!   `layers`) and fans each [`EncodedUnit`] out to the viewers of its layer.
//! - [`pixel::PixelLayers`]: the adapter for today's pixel and JPEG
//!   [`FrameSource`](crate::sim::lane::FrameSource)s (Android, the fake lane,
//!   iOS until the helper encodes, S3). A protocol-4 source (S3's helper,
//!   deskd) implements [`EncodedSource`] by relaying the link's units.
//! - [`link_source::LinkSource`]: that relay (S5): units off a protocol-4
//!   frames pipe, requests through the caller's own link.
//!
//! **Units.** A layer's first unit, and the first after a size or source
//! change, is a key unit with `source_start`. Keyframes otherwise come only
//! on [`EncodedSource::request_keyframe`]; a JPEG unit is always key. `seq`
//! is per layer, +1 per unit, so a viewer that sees a gap knows its decoder
//! chain broke and waits for the next key unit.

pub mod link_source;
pub mod pixel;

use std::fmt;
use std::time::{Duration, Instant};

pub use bytes::Bytes;
pub use k2_simvideo::link::{Codec, LayerSpec};

use crate::sim::lane::{FrameStamps, LaneError};

/// Which source a hub serves. One hub (and one capture) per key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceKey {
    /// A sim session (its row id).
    Sim(i64),
    /// A remote desktop (`desktop` = the desktop id, `console` in v1). One
    /// capture per desktop (v1, K2 11-10): every viewer sees the display
    /// deskd captures, which is a property of the source, not of the key
    /// (`LinkSource::display`). A switch is the controller's and moves every
    /// viewer. Per-viewer displays are a later option.
    Desktop { desktop: String },
}

impl SourceKey {
    pub fn kind(&self) -> SourceKind {
        match self {
            SourceKey::Sim(_) => SourceKind::Sim,
            SourceKey::Desktop { .. } => SourceKind::Desktop,
        }
    }
}

impl fmt::Display for SourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceKey::Sim(id) => write!(f, "s{id}"),
            SourceKey::Desktop { desktop } => write!(f, "desktop {desktop}"),
        }
    }
}

/// The kind of source (the rate table's first key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Sim,
    Desktop,
}

/// A unit's daemon-side encode (the `stats=1` readout's encode stage).
/// `None` on a unit the source encoded itself (a protocol-4 relay).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodeSpan {
    pub start: Instant,
    pub end: Instant,
    /// The encoding thread's CPU time for this encode (0 where unknown).
    pub cpu_ns: u64,
}

/// One encoded unit of one layer, shared by every viewer of that layer
/// (`data` is reference-counted: cloning a unit copies no bytes).
#[derive(Debug, Clone)]
pub struct EncodedUnit {
    pub codec: Codec,
    /// A viewer can start here. Every JPEG unit is key.
    pub key: bool,
    /// The layer's first unit, or the first after a size or source change
    /// (always also `key`).
    pub source_start: bool,
    /// The layer id from the last [`EncodedSource::set_layers`].
    pub layer: u8,
    /// The picture in pixels.
    pub width: u32,
    pub height: u32,
    /// The screen in points for this unit's orientation (the K2SF / K2SV
    /// header's `wpt` / `hpt`; input maps through it).
    pub width_pt: u32,
    pub height_pt: u32,
    /// Per layer, +1 per unit.
    pub seq: u64,
    /// When the device showed it, on the daemon's clock.
    pub captured_at: Instant,
    /// Its time before the daemon had it (helper emit / pipe read).
    pub stamps: FrameStamps,
    /// When the source adapter had it on the daemon (pulled from the lane,
    /// or read off the link).
    pub pulled_at: Instant,
    /// The daemon-side encode, if the daemon encoded it.
    pub encode: Option<EncodeSpan>,
    /// An Annex B access unit (H.264, SPS + PPS before every key unit), a
    /// VP8 frame, or a baseline JPEG.
    pub data: Bytes,
}

/// Why [`EncodedSource::next_unit`] failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    /// The source ended for good (the device stopped, the helper went
    /// away). The hub ends; a new viewer starts a new one.
    Ended(LaneError),
    /// One layer can't go on (its encoder failed to open or encode). The
    /// other layers keep going; this one sends nothing more until a
    /// `set_layers` drops it. The hub moves its viewers to JPEG.
    LayerFailed { layer: u8, why: String },
}

impl SourceError {
    pub fn message(&self) -> &str {
        match self {
            SourceError::Ended(e) => e.message(),
            SourceError::LayerFailed { why, .. } => why,
        }
    }
}

/// A live source of encoded units: one encode per layer. Owned and pulled
/// by one hub's capture thread; dropping it stops the capture.
pub trait EncodedSource: Send {
    /// The next unit of any layer, waiting up to `wait`. `Ok(None)` = none
    /// in that time (a still screen, or every layer paused).
    fn next_unit(&mut self, wait: Duration) -> Result<Option<EncodedUnit>, SourceError>;

    /// The whole layer list (replaces the last). A layer missing from it,
    /// or `on: false`, is paused: no work and no units; one that comes back
    /// starts again with a source-start key unit. A layer keeps its id
    /// while it lives; a known id with new `fps` / `bitrate` / `max_width`
    /// is retuned without a keyframe (a new size brings one).
    fn set_layers(&mut self, layers: &[LayerSpec]);

    /// A key unit on `layer`'s next encode of a frame captured at or after
    /// `since` (absent = the next frame). A still screen sends no newer
    /// frame, so a source may key its newest one after a short wait.
    fn request_keyframe(&mut self, layer: u8, since: Option<Instant>);

    /// The picture's widths for the current orientation, once the source
    /// knows them (`None` before its first frame). The hub sends every
    /// layer an explicit `max_width`: the points width until a viewer on it
    /// sends `pw`, then the widest `pw` clamped to the native width.
    fn sizes(&self) -> Option<SourceSizes>;
}

/// [`EncodedSource::sizes`]: the screen's width in points and, once the
/// source has shown it, in native pixels (a pixel source tuned to points
/// can't tell its native width until it captures native frames).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceSizes {
    pub points: u32,
    pub native_px: Option<u32>,
}

/// This thread's CPU time in ns (the stats readout); 0 where unknown.
#[cfg(unix)]
pub fn thread_cpu_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `ts` is a valid out-pointer and the clock exists on Linux and macOS.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) } != 0 {
        return 0;
    }
    (ts.tv_sec as u64).saturating_mul(1_000_000_000).saturating_add(ts.tv_nsec as u64)
}

#[cfg(not(unix))]
pub fn thread_cpu_ns() -> u64 {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_keys_name_their_kind() {
        assert_eq!(SourceKey::Sim(3).kind(), SourceKind::Sim);
        let d = SourceKey::Desktop { desktop: "console".into() };
        assert_eq!(d.kind(), SourceKind::Desktop);
        assert_eq!(SourceKey::Sim(3).to_string(), "s3");
        assert_eq!(d.to_string(), "desktop console");
        assert_ne!(d, SourceKey::Desktop { desktop: "other".into() }, "one hub per desktop, whatever it displays");
    }
}
