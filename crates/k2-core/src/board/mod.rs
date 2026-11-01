//! The Board (plan-board-garden-v1; Rosson 2026-11-01): the rules agents'
//! `k2 board` writes follow, in Rust. The Board Garden's widget
//! (`k2:board@1`) keeps its boards in its Garden data (`data:k2:board`);
//! this module is the pure part (no store, no daemon): the schema and its
//! typed refusals, templates, ops with the lock rule, `apply` with id
//! mapping, the delete cascade, and the export envelope. The daemon's
//! `board_routes` reads keys, calls these, and writes the result with
//! `ifRev`.
//!
//! **Keys** (format 1):
//! - `boards/<b>` → `{id, name, project, order?, format: 1, trashedAt?, …}`
//! - `projects/<p>` → `{id, name, parent?, order?, trashedAt?, …}`
//! - `board/<b>/meta` → `{format: 1, title, voting?, timer?, batch?}`
//! - `board/<b>/obj/<id>` → the object plus `createdBy`, `createdAt`,
//!   `updatedBy`, `updatedAt` and `batch`.
//!
//! **People** are keyed by a stable id, never a display name:
//! `person:owner` (the person at this computer) and `agent:<handle>`.

pub mod export;
pub mod ops;
pub mod schema;
pub mod templates;

use serde_json::{json, Value};

/// The board data format these rules write.
pub const FORMAT: u64 = 1;

/// The Board widget's data (built-in versions share it).
pub const WIDGET: &str = "k2:board";

/// The person at this computer, as a person key.
pub const PERSON_OWNER: &str = "person:owner";

/// A refusal with a typed code (never a cut value or a skipped op).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardError {
    /// `unknown_type`, `unknown_field`, `bad_field`, `too_long`,
    /// `too_many`, `value_too_large`, `no_object`, `exists`, `locked`,
    /// `no_board`, `board_required`, `board_trashed`, `bad_ops`,
    /// `no_template`, `voting_off`, `no_votes_left`, `bad_request`.
    pub code: &'static str,
    pub message: String,
    pub field: Option<String>,
    pub max: Option<usize>,
}

impl BoardError {
    pub fn new(code: &'static str, message: impl Into<String>) -> BoardError {
        BoardError { code, message: message.into(), field: None, max: None }
    }
    pub fn field(mut self, f: &str) -> BoardError {
        self.field = Some(f.to_string());
        self
    }
    pub fn bad_field(field: &str, why: &str) -> BoardError {
        BoardError::new("bad_field", format!("{field}: {why}")).field(field)
    }
    pub fn too_long(field: &str, max: usize) -> BoardError {
        BoardError { code: "too_long", message: format!("{field}: at most {max} characters (nothing was cut)"), field: Some(field.into()), max: Some(max) }
    }
    pub fn too_many(field: &str, max: usize) -> BoardError {
        BoardError { code: "too_many", message: format!("{field}: at most {max} items (nothing was dropped)"), field: Some(field.into()), max: Some(max) }
    }
    pub fn value_too_large(field: &str, max: usize) -> BoardError {
        BoardError { code: "value_too_large", message: format!("{field}: at most {max} bytes"), field: Some(field.into()), max: Some(max) }
    }
    pub fn unknown_type(ty: &str) -> BoardError {
        BoardError::new("unknown_type", format!("unknown type {ty} (one of {})", schema::type_names().join(", ")))
    }
    /// The wire `error` object.
    pub fn to_json(&self) -> Value {
        let mut v = json!({ "code": self.code, "hint": self.message });
        if let Some(f) = &self.field {
            v["field"] = json!(f);
        }
        if let Some(m) = self.max {
            v["max"] = json!(m);
        }
        v
    }
}

impl std::fmt::Display for BoardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// A key segment (the widget's `seg`): anything outside
/// `[A-Za-z0-9._~:@+-]` becomes `_`.
pub fn seg(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "._~:@+-".contains(c) { c } else { '_' }).collect()
}

/// `board/<b>/`.
pub fn board_prefix(board: &str) -> String {
    format!("board/{}/", seg(board))
}

/// `board/<b>/obj/<id>`.
pub fn obj_key(board: &str, id: &str) -> String {
    format!("board/{}/obj/{}", seg(board), seg(id))
}

/// `board/<b>/meta`.
pub fn meta_key(board: &str) -> String {
    format!("board/{}/meta", seg(board))
}

/// `boards/<b>`.
pub fn row_key(board: &str) -> String {
    format!("boards/{}", seg(board))
}

/// A board id: `b` + 6 lower hex (older boards: 1–40 of `[a-z0-9-]`).
pub fn valid_board_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 40 && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A new board id.
pub fn new_board_id() -> String {
    format!("b{}", schema::hex(6))
}

#[cfg(test)]
mod tests;
