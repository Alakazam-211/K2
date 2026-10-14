//! JSON Pointer projection and aggregates (B1). C0 ships [`Projection`],
//! the shape stored in `source_state.projection_json` (<= 64 KB).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::spec::Json;

/// What one poll kept from the response: source-level fields (named
/// fields and aggregates) and per-item fields, in item order.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Projection {
    #[serde(default)]
    pub fields: BTreeMap<String, Json>,
    /// <= `MAX_ITEMS`.
    #[serde(default)]
    pub items: Vec<ProjectedItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectedItem {
    pub key: String,
    #[serde(default)]
    pub fields: BTreeMap<String, Json>,
}

impl Projection {
    pub fn item(&self, key: &str) -> Option<&ProjectedItem> {
        self.items.iter().find(|i| i.key == key)
    }
}
