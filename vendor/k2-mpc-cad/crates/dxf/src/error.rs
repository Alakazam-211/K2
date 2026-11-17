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

/// A layer the call names isn't in the drawing. (exit 2)
pub const LAYER_MISSING: Code = Code {
    id: "DXF-E020",
    name: "layer_missing",
    exit: 2,
};

/// The entity is of a kind, or in a state, the edit verbs don't change (`entity props` says why). (exit 2)
pub const NOT_EDITABLE: Code = Code {
    id: "DXF-E021",
    name: "entity_not_editable",
    exit: 2,
};

/// `--unit` isn't the drawing's unit. (exit 2)
pub const UNIT_MISMATCH: Code = Code {
    id: "DXF-E022",
    name: "unit_mismatch",
    exit: 2,
};

/// Another object points at an entity a delete would remove, and the pack can't mend it. (exit 2)
pub const REFERENCED: Code = Code {
    id: "DXF-E027",
    name: "referenced",
    exit: 2,
};

/// The edit verbs write DXF 2000 (AC1015) and later only. (exit 2)
pub const VERSION_UNSUPPORTED: Code = Code {
    id: "DXF-E040",
    name: "version_unsupported",
    exit: 2,
};

/// The drawing has no handles, or an entity or object without one, or a handle twice. (exit 2)
pub const HANDLES_OFF: Code = Code {
    id: "DXF-E041",
    name: "handles_off",
    exit: 2,
};

/// The entity or layer belongs to an external reference. (exit 2)
pub const EXTERNAL_REFERENCE: Code = Code {
    id: "DXF-E042",
    name: "external_reference",
    exit: 2,
};

/// The entity is another application's object (a proxy). (exit 2)
pub const PROXY: Code = Code {
    id: "DXF-E043",
    name: "proxy",
    exit: 2,
};

/// The entity is inside a dynamic block's definition or one of its drawn representations. (exit 2)
pub const DYNAMIC_GEOMETRY: Code = Code {
    id: "DXF-E044",
    name: "dynamic_geometry",
    exit: 2,
};

/// The entity, or the layer a change puts it on, is on a locked layer. (exit 2)
pub const LAYER_LOCKED: Code = Code {
    id: "DXF-E045",
    name: "layer_locked",
    exit: 2,
};

/// The entity is inside a block definition: a change there changes every reference. (exit 2)
pub const IN_BLOCK_DEFINITION: Code = Code {
    id: "DXF-E046",
    name: "in_block_definition",
    exit: 2,
};

/// The block reference has no attribute with that tag. (exit 2)
pub const ATTRIB_MISSING: Code = Code {
    id: "DXF-E047",
    name: "attrib_missing",
    exit: 2,
};

/// The attribute's value is a field the app computes. (exit 2)
pub const ATTRIB_DRIVEN: Code = Code {
    id: "DXF-E048",
    name: "attrib_driven",
    exit: 2,
};

/// A multi-line attribute (2018), which this release doesn't write. (exit 2)
pub const ATTRIB_MULTILINE: Code = Code {
    id: "DXF-E049",
    name: "attrib_multiline",
    exit: 2,
};

/// More entities than one call may change. (exit 2)
pub const EDIT_TOO_MANY: Code = Code {
    id: "DXF-E050",
    name: "edit_too_many",
    exit: 2,
};

/// `block insert` names a block it can't place: missing, an external reference, a layout,
/// anonymous, or annotative (`why` in the message). (exit 2)
pub const BLOCK_NOT_INSERTABLE: Code = Code {
    id: "DXF-E051",
    name: "block_not_insertable",
    exit: 2,
};

/// A linetype or text style the call names isn't in its table. (exit 2)
pub const TABLE_ENTRY_MISSING: Code = Code {
    id: "DXF-E053",
    name: "table_entry_missing",
    exit: 2,
};

/// A value the format can't hold in that place. (exit 2)
pub const VALUE_NOT_WRITABLE: Code = Code {
    id: "DXF-E054",
    name: "value_not_writable",
    exit: 2,
};

/// The pack's own read of the edit didn't read back as planned: nothing was planned. (exit 2)
pub const SELF_CHECK: Code = Code {
    id: "DXF-E090",
    name: "self_check",
    exit: 2,
};

/// Every code a DXF read can refuse with: the reader's own, then the shared ones.
pub const ALL: [Code; 6] = [BINARY, NOT_PAIRS, OVER_BOUND, USAGE, NOT_FOUND, AMBIGUOUS];

/// Every code the edit verbs refuse with besides those of [`ALL`].
pub const EDIT: [Code; 19] = [
    LAYER_MISSING,
    NOT_EDITABLE,
    UNIT_MISMATCH,
    REFERENCED,
    VERSION_UNSUPPORTED,
    HANDLES_OFF,
    EXTERNAL_REFERENCE,
    PROXY,
    DYNAMIC_GEOMETRY,
    LAYER_LOCKED,
    IN_BLOCK_DEFINITION,
    ATTRIB_MISSING,
    ATTRIB_DRIVEN,
    ATTRIB_MULTILINE,
    EDIT_TOO_MANY,
    BLOCK_NOT_INSERTABLE,
    TABLE_ENTRY_MISSING,
    VALUE_NOT_WRITABLE,
    SELF_CHECK,
];
