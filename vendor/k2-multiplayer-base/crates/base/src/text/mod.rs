//! Text algorithms: minimal diff to edits, and the three-way merge used for writes made
//! against an older version and for per-participant undo. Engine-independent.

pub mod diff;
pub mod merge;

pub use diff::{Hunk, HunkError};
