//! Why a translator refused a file, typed, and the bounds it reads under.

use crate::time::TimeError;

/// Longest piece of file text a refusal quotes (a version, a schema or element name).
pub const QUOTE_MAX: usize = 64;

/// Why a translator refused a file. A refusal never quotes more than [`QUOTE_MAX`] bytes of
/// the file, and [`ReadError::reason`] quotes none.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    /// Over [`ReadLimits::max_bytes`] (for a compressed file, its decompressed size).
    #[error("over the reader's limit of {limit} bytes")]
    TooLarge {
        /// The limit.
        limit: u64,
    },
    /// Not well-formed: bad JSON, bad XML, a damaged compressed stream.
    #[error("malformed {format} at byte {at}: {why}")]
    Syntax {
        /// The layer that failed (`json`, `xml`, `gzip`, `utf-8`).
        format: &'static str,
        /// Where, in the bytes that layer read.
        at: u64,
        /// What was wrong (fixed text).
        why: &'static str,
    },
    /// A version of the format, or of one of its object types, this reader doesn't know.
    #[error("{what} version {found:?} is not one this reader knows")]
    UnknownVersion {
        /// What carries the version (`document`, `OTIO_SCHEMA Clip`, `class Sequence`).
        what: String,
        /// The version the file has, cut to [`QUOTE_MAX`] bytes.
        found: String,
    },
    /// An object type, element or class this reader doesn't know, where the reader needs to
    /// know it (a track's child, a story element).
    #[error("{what} {found:?} is not one this reader knows")]
    Unknown {
        /// What kind of thing (`OTIO schema`, `element`, `track item class`).
        what: &'static str,
        /// Its name in the file, cut to [`QUOTE_MAX`] bytes.
        found: String,
    },
    /// Well-formed, but not a structure this reader reads: a required field missing, a
    /// reference to nothing, a time that runs backwards.
    #[error("not a structure this reader reads: {why}")]
    Structure {
        /// What was wrong (fixed text).
        why: &'static str,
    },
    /// A time or rate in the file this reader can't hold exactly.
    #[error("a time in the file: {0}")]
    Time(#[from] TimeError),
    /// Over one of the reader's other bounds.
    #[error("over the reader's limit of {limit} {what}")]
    Limit {
        /// Which bound (`nesting levels`, `objects`).
        what: &'static str,
        /// The limit.
        limit: u64,
    },
    /// Reading the bytes failed (a pack reading from a stream).
    #[error("reading the file: {0}")]
    Io(#[from] std::io::Error),
}

impl ReadError {
    /// The refusal as fixed text, quoting nothing from the file: what a pack hands the kit.
    pub fn reason(&self) -> &'static str {
        match self {
            ReadError::TooLarge { .. } => "over the reader's size limit",
            ReadError::Syntax { .. } => "the file is malformed",
            ReadError::UnknownVersion { .. } => "a version this reader doesn't know",
            ReadError::Unknown { .. } => "an object type this reader doesn't know",
            ReadError::Structure { why } => why,
            ReadError::Time(_) => "a time this reader can't hold exactly",
            ReadError::Limit { .. } => "over one of the reader's limits",
            ReadError::Io(_) => "reading the file failed",
        }
    }
}

/// `s` cut to [`QUOTE_MAX`] bytes at a character boundary.
pub(crate) fn quote(s: &str) -> String {
    let mut at = s.len().min(QUOTE_MAX);
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    s[..at].to_owned()
}

/// The bounds a translator reads under. A file over one is refused, never read in part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadLimits {
    /// Most bytes of the document (decompressed, for a compressed file).
    pub max_bytes: u64,
    /// Deepest nesting the reader follows (JSON values, XML elements, nested timelines).
    pub max_depth: u32,
    /// Most objects: JSON values, XML nodes, or model nodes.
    pub max_items: u64,
}

impl ReadLimits {
    /// The bounds, as given.
    pub const fn new(max_bytes: u64, max_depth: u32, max_items: u64) -> Self {
        Self {
            max_bytes,
            max_depth,
            max_items,
        }
    }
}

impl Default for ReadLimits {
    /// 256 MiB, 128 levels, 4 million objects.
    fn default() -> Self {
        Self::new(256 << 20, 128, 4_000_000)
    }
}
