//! A format-neutral model of a 2D CAD drawing, for the multiplayer kit: the model a drawing
//! pack's reader fills ([`Doc`]: entities by handle, layers, blocks, attributes, external
//! references), the structure index built from it ([`index`]), and the reads every drawing
//! format shares ([`reads`]).
//!
//! A reader for a format (a tokenizer, a bit reader) turns a version's bytes into a [`Doc`],
//! with each entity's facts as separate fields and digests; everything after that is here, so
//! every format indexes, finds and counts the same way.
//!
//! **A node's attributes are its own record's facts, never counts over other records.** So two
//! people's edits to different entities change different nodes, and an add on a layer changes
//! neither the layer's node nor any other.
#![forbid(unsafe_code)]

pub mod args;
pub mod change;
pub mod draw;
pub mod error;
pub mod geom;
pub mod handle;
pub mod index;
pub mod leader;
pub mod merge;
pub mod model;
pub mod mutant;
pub mod probe;
pub mod reads;

pub use error::{CadError, Code};
pub use model::{
    Attrib, BboxQ, BlockKind, BlockRec, Doc, Entity, Extra, Interner, Layer, Props, Shared, Units,
};
