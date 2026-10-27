//! Text algorithms: minimal diff to edits, and the three-way merge used for writes made
//! against an older version and for per-participant undo. Engine-independent.

pub mod diff;
pub mod lines;
pub mod merge;
pub(crate) mod myers;

pub use diff::{Hunk, HunkError};
