//! Reads PCB fabrication outputs, read-only, for the multiplayer kit: photoplot files and drill
//! files in the XNC subset, and reads common drill exports.
//!
//! - [`photoplot`]: a strict reader for the current revision (2026.05) of the photoplot format's
//!   specification, into a model of apertures, macros, objects with their polarity, and
//!   attributes, every coordinate an integer in the file's own units.
//! - [`drill`]: a strict reader for the XNC subset (revision 2021.11): header, tools, hits, rout
//!   paths and attributes; and [`drill::common`], a separate reading of common drill exports,
//!   whose every file is reported not conforming with the constructs it relied on.
//! - [`pack`]: each reader as the kit's tier-0 blob pack, with a structure index: families of
//!   apertures, objects grouped by aperture and by attribute, regions, tools and their holes.
//!
//! A file that doesn't conform is refused with a typed [`error::ReadError`] (or, where the
//! specification lets a reader read on, read with a typed [`error::Warning`]); nothing is
//! guessed. Nothing here writes either format. Specification sections are cited by number only.
#![forbid(unsafe_code)]

pub mod attr;
pub mod dec;
pub mod drill;
pub mod error;
#[cfg(feature = "fuzzing")]
pub mod fuzzing;
pub mod geom;
pub mod mutant;
pub mod pack;
pub mod photoplot;

pub use error::{Code, Conformance, ReadError, Warning};
pub use pack::{
    index_common, index_drill, index_photoplot, CommonDrillPack, DrillPack, PhotoplotPack,
};

/// The resources a read may use; over any, a read is refused with
/// [`error::Code::Limit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// The file's size.
    pub max_bytes: u64,
    /// One command (a word) or line.
    pub max_word_bytes: usize,
    /// Graphical objects (photoplot) or hits and rout paths (drill), before any repeat.
    pub max_objects: u64,
    /// Objects once step and repeat and block flashes are expanded.
    pub max_expansion: u64,
    /// Objects, region segments and rout segments together.
    pub max_points: u64,
    /// Apertures, block apertures included.
    pub max_apertures: u64,
    /// Aperture macros.
    pub max_macros: u64,
    /// Nesting in one macro expression.
    pub max_expr_depth: usize,
    /// Expression nodes evaluated for all macro apertures.
    pub max_macro_work: u64,
    /// Nested block apertures.
    pub max_block_depth: usize,
    /// Distinct attribute states objects take.
    pub max_attr_sets: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_bytes: 64 << 20,
            max_word_bytes: 1 << 20,
            max_objects: 4_000_000,
            max_expansion: 100_000_000,
            max_points: 16_000_000,
            max_apertures: 100_000,
            max_macros: 10_000,
            max_expr_depth: 64,
            max_macro_work: 50_000_000,
            max_block_depth: 32,
            max_attr_sets: 1_000_000,
        }
    }
}
