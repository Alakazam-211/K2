//! The DWG reader's refusals (`DWG-E…`), beside the ones every drawing format shares
//! (`k2_mpc_cad_model::error`). A refusal's message never quotes file content.

use k2_mpc_cad_model::error::Code;

/// A read ran past the end of its bytes: the file is cut short or damaged (exit 2).
pub const TRUNCATED: Code = Code {
    id: "DWG-E002",
    name: "truncated",
    exit: 2,
};

/// A value no writer makes: a code with no meaning, a handle longer than 8 bytes, a modular
/// number longer than its type (exit 2).
pub const BAD_VALUE: Code = Code {
    id: "DWG-E003",
    name: "bad_value",
    exit: 2,
};

/// A compressed stream that can't be decompressed: an opcode out of place, a match reaching
/// before the start of the page (exit 2).
pub const BAD_STREAM: Code = Code {
    id: "DWG-E004",
    name: "bad_stream",
    exit: 2,
};

/// Over a reader limit ([`crate::Limits`]), or a page decompressing past its declared size
/// (exit 2).
pub const OVER_BOUND: Code = Code {
    id: "DWG-E005",
    name: "over_bound",
    exit: 2,
};

/// The container doesn't hold together: a header id or CRC, a page's type or checksum, a map
/// entry naming a page that isn't there, the header's two copies differing (exit 2).
pub const BAD_CONTAINER: Code = Code {
    id: "DWG-E006",
    name: "bad_container",
    exit: 2,
};

/// An encrypted or password-protected drawing: refused, never read (exit 2).
pub const ENCRYPTED: Code = Code {
    id: "DWG-E007",
    name: "encrypted",
    exit: 2,
};

/// A DWG version outside the 2004 to 2018 family (exit 2).
pub const UNSUPPORTED_VERSION: Code = Code {
    id: "DWG-E008",
    name: "unsupported_version",
    exit: 2,
};

/// Not a DWG file: no version stamp (exit 2).
pub const NOT_DWG: Code = Code {
    id: "DWG-E009",
    name: "not_dwg",
    exit: 2,
};

/// Every code the DWG reader can refuse with.
pub const ALL: [Code; 8] = [
    TRUNCATED,
    BAD_VALUE,
    BAD_STREAM,
    OVER_BOUND,
    BAD_CONTAINER,
    ENCRYPTED,
    UNSUPPORTED_VERSION,
    NOT_DWG,
];
