//! The DXF reader's own refusals (`DXF-E…`), beside the shared ones in
//! [`k2_mpc_cad_model::error`] (`CAD-E…`).

pub use k2_mpc_cad_model::error::{CadError, Code, AMBIGUOUS, NOT_FOUND, USAGE};

/// The file is binary DXF (exit 2).
pub const BINARY: Code = Code {
    id: "DXF-E001",
    name: "binary_dxf",
    exit: 2,
};

/// The file isn't code/value pairs: an odd line count, a code that isn't an integer (exit 2).
pub const NOT_PAIRS: Code = Code {
    id: "DXF-E002",
    name: "not_dxf",
    exit: 2,
};

/// Over a reader limit ([`crate::Limits`]): a line, an entity, the pair count, the size (exit 2).
pub const OVER_BOUND: Code = Code {
    id: "DXF-E003",
    name: "over_bound",
    exit: 2,
};

/// Every code a DXF read can refuse with: the reader's own, then the shared ones.
pub const ALL: [Code; 6] = [BINARY, NOT_PAIRS, OVER_BOUND, USAGE, NOT_FOUND, AMBIGUOUS];
