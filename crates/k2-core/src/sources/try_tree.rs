//! The shape tree `POST /cli/sources/try` shows (contract §4.3). Owner:
//! B2. C0 ships the node type and the signature.

use serde::Serialize;

use super::spec::Json;

/// At most this many nodes; arrays show their first 3 elements.
pub const MAX_NODES: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeNode {
    pub pointer: String,
    /// `object` | `array` | `string` | `number` | `bool` | `null`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<Json>,
}

/// `(nodes, truncated)`.
///
/// C0 skeleton: an empty tree (B2 replaces it).
pub fn build(body: &Json) -> (Vec<TreeNode>, bool) {
    let _ = body;
    (Vec::new(), false)
}
