//! Refusals: each with a stable id, a name, the kit's exit code and a next step. The ids shared
//! by every drawing format are here (`CAD-E…`); a format's reader adds its own beside them. A
//! refusal's message never quotes file content.

use std::fmt;

/// What was refused: its stable id, its name (snake case) and the kit's exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Code {
    pub id: &'static str,
    pub name: &'static str,
    pub exit: u8,
}

/// A verb's arguments can't be read (exit 2).
pub const USAGE: Code = Code {
    id: "CAD-E004",
    name: "usage",
    exit: 2,
};

/// An address names nothing (exit 4).
pub const NOT_FOUND: Code = Code {
    id: "CAD-E010",
    name: "address_not_found",
    exit: 4,
};

/// A name matches two records (exit 2).
pub const AMBIGUOUS: Code = Code {
    id: "CAD-E011",
    name: "name_ambiguous",
    exit: 2,
};

/// An address that should name a closed polyline (a boundary) doesn't (exit 2).
pub const NOT_A_BOUNDARY: Code = Code {
    id: "CAD-E012",
    name: "not_a_boundary",
    exit: 2,
};

/// A page cursor from another version or space (exit 2).
pub const STALE_CURSOR: Code = Code {
    id: "CAD-E013",
    name: "stale_cursor",
    exit: 2,
};

/// A refusal: its code, where in the file (for a reading refusal), why, and what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CadError {
    pub code: Code,
    /// The byte offset the refusal is about, for a reading refusal.
    pub offset: Option<u64>,
    /// Why, in words; never file content.
    pub why: String,
    /// What to do next.
    pub next: String,
}

impl CadError {
    pub fn new(code: Code, why: impl Into<String>, next: impl Into<String>) -> Self {
        CadError {
            code,
            offset: None,
            why: why.into(),
            next: next.into(),
        }
    }

    /// The same refusal, about byte `offset`.
    pub fn at(mut self, offset: usize) -> Self {
        self.offset = Some(offset as u64);
        self
    }

    /// One line, as the kit stores an index refusal: id, name, offset and why.
    pub fn summary(&self) -> String {
        match self.offset {
            Some(o) => format!(
                "{} {} at byte {o}: {}",
                self.code.id, self.code.name, self.why
            ),
            None => format!("{} {}: {}", self.code.id, self.code.name, self.why),
        }
    }
}

impl fmt::Display for CadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.summary())
    }
}

impl std::error::Error for CadError {}
