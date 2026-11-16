//! What reading a file can refuse ([`ReadError`]) and the bounds a read stays under
//! ([`ReadLimits`]). A refusal's text is fixed: it never quotes anything from the file.

use std::io;

/// The bounds one read stays under. A file over any of them is refused with
/// [`ReadError::Limit`], never read in part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReadLimits {
    /// Most bytes of the file after decompression (headers included).
    pub max_raw_bytes: u64,
    /// Most file blocks.
    pub max_blocks: u64,
    /// Most bytes of block bodies kept in memory at once (every data-block's own struct, and
    /// small data blocks while they fit). Larger bodies are digested as they pass and dropped.
    pub max_retained_bytes: u64,
    /// The largest data block kept whole; a larger one is digested and dropped.
    pub retain_block_max: u64,
    /// The largest schema block (`DNA1`).
    pub max_sdna_bytes: u64,
    /// The largest zstd window a compressed file may ask for: the decoder's buffer.
    pub max_window_bytes: u64,
}

impl ReadLimits {
    /// Limits with every field given.
    pub const fn new(
        max_raw_bytes: u64,
        max_blocks: u64,
        max_retained_bytes: u64,
        retain_block_max: u64,
        max_sdna_bytes: u64,
        max_window_bytes: u64,
    ) -> Self {
        Self {
            max_raw_bytes,
            max_blocks,
            max_retained_bytes,
            retain_block_max,
            max_sdna_bytes,
            max_window_bytes,
        }
    }
}

impl Default for ReadLimits {
    /// 64 GiB of file, 4 million blocks, 256 MiB kept, data blocks up to 64 KiB kept, a schema up
    /// to 16 MiB, zstd windows up to 64 MiB.
    fn default() -> Self {
        Self::new(64 << 30, 4_000_000, 256 << 20, 64 << 10, 16 << 20, 64 << 20)
    }
}

/// Why a file can't be read. Every reason is fixed text: nothing from the file.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ReadError {
    /// The stream failed: a damaged blob or a broken pipe, not a bad file.
    #[error("reading the file: {0}")]
    Io(io::Error),
    /// Not a BLEND file: no file header, and not a compressed one either.
    #[error("not a BLEND file")]
    NotBlend,
    /// A BLEND file in a form this reader doesn't know: refused rather than guessed.
    #[error("an unknown BLEND file form: {what}")]
    Unknown {
        /// The part of the header that is unknown (fixed text).
        what: &'static str,
    },
    /// The file ends before its last block.
    #[error("the file ends early: {what}")]
    Truncated {
        /// Where it ends (fixed text).
        what: &'static str,
    },
    /// The file breaks its own structure.
    #[error("the file is malformed: {what}")]
    Malformed {
        /// What is wrong (fixed text).
        what: &'static str,
    },
    /// The file is over one of the [`ReadLimits`].
    #[error("over a reader limit: {what} (limit {limit})")]
    Limit {
        /// Which bound (fixed text).
        what: &'static str,
        /// The bound.
        limit: u64,
    },
}

impl ReadError {
    /// The fixed reason, for a refusal that may be stored or shown.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Io(_) => "the stream failed",
            Self::NotBlend => "not a BLEND file",
            Self::Unknown { what }
            | Self::Truncated { what }
            | Self::Malformed { what }
            | Self::Limit { what, .. } => what,
        }
    }

    pub(crate) fn malformed(what: &'static str) -> Self {
        Self::Malformed { what }
    }

    pub(crate) fn truncated(what: &'static str) -> Self {
        Self::Truncated { what }
    }
}
