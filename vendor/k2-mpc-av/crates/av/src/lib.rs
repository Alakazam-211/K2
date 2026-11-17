//! Reads a set of AV system drawings the way a technician reads them, over the format-neutral
//! drawing model of `k2-mpc-cad`:
//!
//! - [`config`]: the project's conventions (attribute tags, layers, linetypes, block names), from
//!   a configuration file the caller passes. Nothing about a project is built in.
//! - [`set`]: the reading of a set of sheets: devices across sheets, boundaries and the pulls a
//!   cable crossing one implies, off-sheet flag pairs, title-block revisions and the sheet index,
//!   with findings for what doesn't hold together (`av.set/1`).
//!
//! It never writes a drawing.
#![forbid(unsafe_code)]

pub mod config;
pub mod set;

pub use config::Config;
pub use set::{read_files, read_set, Sheet};

use k2_mpc_cad_model::error::Code;

/// A configuration that can't be read (exit 2).
pub const BAD_CONFIG: Code = Code {
    id: "AV-E001",
    name: "bad_config",
    exit: 2,
};

/// A sheet that can't be read (exit 2).
pub const SHEET_UNREADABLE: Code = Code {
    id: "AV-E002",
    name: "sheet_unreadable",
    exit: 2,
};
