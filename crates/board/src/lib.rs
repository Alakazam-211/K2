//! The `.k5board` file format: a board model, a strict zip reader and writer, canonical JSON, and
//! typed refusals. See `docs/FORMAT.md`.
//!
//! The crate works on bytes. A host owns the file: where it lives, replacing it atomically, and
//! comparing the save guard. There is no clock, no random source and no thread here: a host
//! supplies the random bytes for a lineage or a person token.
//!
//! ```no_run
//! # use k2_mpc_board::*;
//! # fn demo(file: std::fs::File) -> Result<(), Refusal> {
//! let mut pkg = Package::open(file, &Limits::V1)?;
//! let guard = pkg.package_guard();
//! let mut board = pkg.board().clone();
//! board.settings.insert("title".into(), json::Value::Str("Plan".into()));
//! let meta = pkg.manifest().write_meta();
//! let mut out = Vec::new();
//! let written = write(&mut out, &meta, &board, &mut pkg, &Limits::V1)?;
//! // Before replacing the file: compare package_guard_of(the file on disk) with `guard`.
//! # let _ = (guard, written); Ok(()) }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod bad_ids;
pub mod conformance;
mod hash;
pub mod json;
mod limits;
mod model;
mod ops;
mod package;
mod parts;
mod refusal;
pub mod schema;
mod validate;
mod zip;

pub use hash::Sha256;
pub use json::{Fields, Value};
pub use limits::Limits;
pub use model::{
    mint_person_token, Board, Id, Lineage, Object, ObjectType, Person, RefState, Reference,
};
pub use ops::{apply_group, Conflict, Group, Moved, Op, OrderList, Position, Report, Target};
pub use package::{
    package_guard_of, write, Base, Blob, Manifest, Package, Part, WriteMeta, Written,
};
pub use parts::{MemParts, Overlay, PartSource};
pub use refusal::{Refusal, RefusalCode};
pub use schema::schema_json;

/// The manifest's `format`.
pub const FORMAT: &str = "k5board";
/// The format version this crate writes.
pub const VERSION: u32 = 1;
/// The oldest reader version that can read what this crate writes, and the version it reads.
pub const MIN_READER: u32 = 1;
/// The file's media type.
pub const MEDIA_TYPE: &str = "application/vnd.k5.board+zip";
