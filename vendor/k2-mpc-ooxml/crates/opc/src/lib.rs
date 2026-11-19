//! The Open Packaging Conventions layer shared by the docx, pptx and xlsx packs of the K2
//! multiplayer kit: reading a package (its zip, parts, content types and relationships),
//! writing it back by splicing only the parts that changed, and a three-way merge of two saves
//! at the package level that re-mints colliding part names and relationship ids.
//!
//! - [`zip`]: a bounded zip reader that accepts only what it can reproduce exactly, and a
//!   writer that copies untouched entries byte for byte (its module documentation says what a
//!   byte-exact round trip needs).
//! - [`OpcPackage`]: an open package; [`OpcPackage::write`] splices changes in, and
//!   [`OpcPackage::write_plan`] gives the same write as ranges of the file's bytes.
//! - [`types`] and [`rels`]: `[Content_Types].xml` and relationships parts, with the byte ranges
//!   of what they hold.
//! - [`merge`]: the package-level three-way merge; a format pack plugs in its own content
//!   merge with [`merge::PartMerge`].
//!
//! Everything here reads untrusted bytes. Nothing panics on them: a package this layer can't
//! hold is a typed [`Decline`].
#![forbid(unsafe_code)]

mod error;
pub mod limits;
pub mod merge;
pub mod mutant;
pub mod name;
pub mod package;
pub mod rels;
pub mod types;
pub mod xml;
pub mod zip;

pub use error::Decline;
pub use limits::Limits;
pub use package::{Change, OpcPackage, Part};
pub use zip::Span;
