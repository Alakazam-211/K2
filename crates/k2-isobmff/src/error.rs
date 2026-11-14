//! Errors, limits and four-character codes.

use std::fmt;

/// A four-character code (box type, sample entry format, handler type).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    /// From a 4-byte literal: `FourCC::new(b"moov")`.
    pub const fn new(b: &[u8; 4]) -> FourCC {
        FourCC(*b)
    }
    /// From a big-endian `u32`.
    pub const fn from_u32(v: u32) -> FourCC {
        FourCC(v.to_be_bytes())
    }
    /// As a big-endian `u32`.
    pub const fn as_u32(self) -> u32 {
        u32::from_be_bytes(self.0)
    }
}

impl fmt::Display for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.iter().all(|b| (0x20..0x7f).contains(b)) {
            for &b in &self.0 {
                write!(f, "{}", b as char)?;
            }
            Ok(())
        } else {
            write!(f, "0x{:08x}", self.as_u32())
        }
    }
}

impl fmt::Debug for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FourCC({self})")
    }
}

/// Which counted limit was hit (see [`Limits`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// [`Limits::max_depth`].
    Depth,
    /// [`Limits::max_boxes`].
    Boxes,
    /// [`Limits::max_tracks`].
    Tracks,
    /// [`Limits::max_samples_per_track`].
    TrackSamples,
    /// [`Limits::max_total_samples`].
    TotalSamples,
    /// [`Limits::max_entries`].
    Entries,
    /// [`Limits::max_fragments`].
    Fragments,
}

/// Every way reading or writing can fail. Each malformed input maps to one
/// variant; nothing panics.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Error {
    /// The input failed to read or seek (the kind only, so errors compare).
    Io(std::io::ErrorKind),
    /// A box's payload ended before a field it must hold.
    Truncated(FourCC),
    /// A box declares a size smaller than its own header.
    BadBoxSize { fourcc: FourCC, size: u64 },
    /// A child box runs past the end of its parent (overlapping boxes).
    PastParent { fourcc: FourCC },
    /// A box, a table or a sample runs past the end of the file.
    PastEndOfFile { fourcc: FourCC, end: u64, file_len: u64 },
    /// A box we load into memory is larger than [`Limits::max_box_bytes`]
    /// (or [`Limits::max_table_bytes`] for a sample table), refused before
    /// allocating.
    TooLarge { fourcc: FourCC, size: u64, limit: u64 },
    /// A counted limit was reached.
    LimitReached(Limit),
    /// A box that must be present is missing (`parent`/`child`).
    Missing { parent: FourCC, child: FourCC },
    /// A box that may appear once appears twice.
    Duplicate(FourCC),
    /// A box version this reader does not know.
    UnsupportedVersion { fourcc: FourCC, version: u8 },
    /// A field holds a value the format does not allow.
    Invalid { fourcc: FourCC, what: &'static str },
    /// The sample tables of a track disagree with each other.
    TableMismatch { track_id: u32, what: &'static str },
    /// A sample's bytes lie (partly) outside the file.
    SampleOutOfFile { track_id: u32, sample: u64 },
    /// A fragment names a track the movie does not have.
    UnknownTrack(u32),
    /// A time or offset sum overflowed 64 bits.
    Overflow(FourCC),
    /// No `moov` box at the top level.
    NoMovie,
    /// The writer was given inconsistent input.
    Writer(&'static str),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e.kind())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io(k) => write!(f, "i/o error: {k:?}"),
            Error::Truncated(c) => write!(f, "{c}: truncated"),
            Error::BadBoxSize { fourcc, size } => write!(f, "{fourcc}: bad box size {size}"),
            Error::PastParent { fourcc } => write!(f, "{fourcc}: runs past its parent"),
            Error::PastEndOfFile { fourcc, end, file_len } => write!(f, "{fourcc}: ends at {end}, past the file's {file_len} bytes"),
            Error::TooLarge { fourcc, size, limit } => write!(f, "{fourcc}: {size} bytes is over the limit of {limit}"),
            Error::LimitReached(l) => write!(f, "limit reached: {l:?}"),
            Error::Missing { parent, child } => write!(f, "{parent}: no {child}"),
            Error::Duplicate(c) => write!(f, "{c}: duplicated"),
            Error::UnsupportedVersion { fourcc, version } => write!(f, "{fourcc}: unsupported version {version}"),
            Error::Invalid { fourcc, what } => write!(f, "{fourcc}: {what}"),
            Error::TableMismatch { track_id, what } => write!(f, "track {track_id}: {what}"),
            Error::SampleOutOfFile { track_id, sample } => write!(f, "track {track_id}: sample {sample} lies outside the file"),
            Error::UnknownTrack(id) => write!(f, "fragment for unknown track {id}"),
            Error::Overflow(c) => write!(f, "{c}: arithmetic overflow"),
            Error::NoMovie => write!(f, "no moov box"),
            Error::Writer(w) => write!(f, "writer: {w}"),
        }
    }
}

impl std::error::Error for Error {}

/// Shorthand.
pub type Result<T> = std::result::Result<T, Error>;

/// Bounds on what a reader accepts, for untrusted input. The defaults are
/// conservative for proxies and camera originals of a few hours; a host can
/// lower or raise each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Box nesting depth (top level is depth 1). Default 16.
    pub max_depth: u32,
    /// Boxes visited in the whole file, top level included. Default 1,000,000.
    pub max_boxes: u64,
    /// Bytes of any box read into memory (`moov`, `moof`, `sidx`, …).
    /// Default 64 MiB.
    pub max_box_bytes: u64,
    /// Bytes of any one sample table (`stsz`, `stco`, …). Default 32 MiB.
    pub max_table_bytes: u64,
    /// Entries in any one table or list (edit lists, `stsc`, `trun`, …).
    /// Default 16,777,216.
    pub max_entries: u64,
    /// Tracks in the movie. Default 64.
    pub max_tracks: u32,
    /// Samples in one track (moov tables and fragments together). Default
    /// 16,777,216. QuickTime PCM tracks often count one sample per PCM
    /// frame: a host reading long PCM must raise this.
    pub max_samples_per_track: u64,
    /// Samples across all tracks. Default 33,554,432.
    pub max_total_samples: u64,
    /// `moof` boxes. Default 200,000.
    pub max_fragments: u64,
    /// Bytes of one sample [`Mp4Reader::read_sample`](crate::Mp4Reader::read_sample)
    /// will read. Default 256 MiB.
    pub max_sample_bytes: u64,
}

impl Default for Limits {
    fn default() -> Limits {
        Limits {
            max_depth: 16,
            max_boxes: 1_000_000,
            max_box_bytes: 64 << 20,
            max_table_bytes: 32 << 20,
            max_entries: 1 << 24,
            max_tracks: 64,
            max_samples_per_track: 1 << 24,
            max_total_samples: 1 << 25,
            max_fragments: 200_000,
            max_sample_bytes: 256 << 20,
        }
    }
}
