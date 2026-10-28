//! The store interface (contract §3.3; PRD §5.1 `StoreBackend`). C0 → B1.
//!
//! Garden data uses [`super::sqlite::SqliteStore`] on every platform. 1B's
//! workspace documents may implement the same trait over Postgres later.
//! Every method returns its errors; nothing is swallowed (GL5).

use serde::{Deserialize, Serialize};

use super::{Actor, DataError, Space};

pub type Json = serde_json::Value;

/// One live key. `by` is the stored actor (`Actor::stored`); the routes
/// turn it into the wire form (contract §5.3).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub key: String,
    pub rev: i64,
    pub value: Json,
    pub updated_at: i64,
    pub by: String,
}

/// One write in a batch, a transaction part or an import.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Op {
    Put {
        key: String,
        value: Json,
        #[serde(default, rename = "ifRev", skip_serializing_if = "Option::is_none")]
        if_rev: Option<i64>,
    },
    Delete {
        key: String,
        #[serde(default, rename = "ifRev", skip_serializing_if = "Option::is_none")]
        if_rev: Option<i64>,
    },
    /// A field-level merge (0.46.5, Board via Appa): `value` is the
    /// partial, merged exactly like `store.patch` (`patch::merged`, RFC
    /// 7396; null removes a field) inside the same all-or-nothing write.
    Patch {
        key: String,
        value: Json,
        #[serde(default, rename = "ifRev", skip_serializing_if = "Option::is_none")]
        if_rev: Option<i64>,
    },
}

impl Op {
    pub fn key(&self) -> &str {
        match self {
            Op::Put { key, .. } | Op::Delete { key, .. } | Op::Patch { key, .. } => key,
        }
    }

    pub fn if_rev(&self) -> Option<i64> {
        match self {
            Op::Put { if_rev, .. } | Op::Delete { if_rev, .. } | Op::Patch { if_rev, .. } => *if_rev,
        }
    }

    /// Canonical JSON bytes of the value the op carries (0 for a delete; a
    /// patch's partial, not the merged document).
    pub fn value_bytes(&self) -> usize {
        match self {
            Op::Put { value, .. } | Op::Patch { value, .. } => {
                serde_json::to_string(value).map(|s| s.len()).unwrap_or(usize::MAX)
            }
            Op::Delete { .. } => 0,
        }
    }
}

/// One `list` page, in key order. `next` is the last key returned (an
/// exclusive cursor), or `None` at the end.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Page {
    pub items: Vec<Entry>,
    pub next: Option<String>,
}

/// One change-feed row. `value` is null for a delete.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub key: String,
    pub rev: i64,
    pub deleted: bool,
    pub value: Json,
    pub by: String,
    pub at: i64,
    /// The restore that wrote this change (prd-garden-board-needs-v1 GH9);
    /// `None` for an ordinary write.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore: Option<String>,
}

/// A position in the feed: rows strictly after `(since, after)` in
/// `(rev, key)` order. `after = None` means `rev > since`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub since: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
}

/// One `changes` page.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangesPage {
    /// The space's rev now.
    pub head: i64,
    /// Ordered by `(rev, key)`.
    pub items: Vec<Change>,
    /// Some when more rows exist after this page.
    pub next: Option<Cursor>,
    /// The page ends inside a rev: fetch `next` before applying.
    pub partial: bool,
}

/// One space's counters (`spaces` row).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceInfo {
    pub space: String,
    pub widget: String,
    pub rev: i64,
    pub reaped_rev: i64,
    pub bytes: i64,
    pub keys: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub orphaned_at: Option<i64>,
}

/// What one reaper pass removed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReapReport {
    pub tombstones: usize,
    pub log_rows: usize,
}

/// A read filter: `None` = everything; `Some(prefixes)` = only keys under
/// one of them (an agent's read prefixes). Applied inside the paging loop.
pub type Filter<'a> = Option<&'a [String]>;

/// The Garden data store (contract §3.3).
pub trait StoreBackend: Send + Sync {
    fn get(&self, space: &Space, key: &str) -> Result<Option<Entry>, DataError>;
    /// One op, one new rev. Answers the new rev.
    fn put(&self, space: &Space, key: &str, value: &Json, if_rev: Option<i64>, by: &Actor) -> Result<i64, DataError>;
    /// `Some(new rev)`, or `None` when the key didn't exist (no rev bump).
    fn delete(&self, space: &Space, key: &str, if_rev: Option<i64>, by: &Actor) -> Result<Option<i64>, DataError>;
    /// All-or-nothing, one new rev for the whole set (batch, tx commit,
    /// import). A conflict or a refused patch op names the op's index.
    fn apply(&self, space: &Space, ops: &[Op], by: &Actor) -> Result<i64, DataError>;
    fn list(
        &self,
        space: &Space,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        filter: Filter,
    ) -> Result<Page, DataError>;
    fn changes(&self, space: &Space, from: &Cursor, prefix: &str, filter: Filter) -> Result<ChangesPage, DataError>;
    fn info(&self, space: &Space) -> Result<Option<SpaceInfo>, DataError>;
    fn spaces(&self) -> Result<Vec<SpaceInfo>, DataError>;
    fn drop_space(&self, space: &Space) -> Result<(), DataError>;
    fn move_space(&self, from: &Space, to: &Space) -> Result<(), DataError>;
    /// Mark `data:` spaces whose widget isn't in `live_widgets`; clear the
    /// mark on the ones that are. Answers the widgets newly marked.
    fn mark_orphans(&self, live_widgets: &[String], now: i64) -> Result<Vec<String>, DataError>;
    fn reap(&self, now: i64) -> Result<ReapReport, DataError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ops_parse_from_the_wire_shape() {
        let ops: Vec<Op> = serde_json::from_str(
            r#"[{"op":"put","key":"obj/1","value":{"x":1},"ifRev":0},{"op":"delete","key":"obj/2"}]"#,
        )
        .expect("ops parse");
        assert_eq!(ops[0], Op::Put { key: "obj/1".into(), value: serde_json::json!({"x": 1}), if_rev: Some(0) });
        assert_eq!(ops[1].key(), "obj/2");
        assert_eq!(ops[1].value_bytes(), 0);
        assert!(serde_json::from_str::<Op>(r#"{"op":"put","key":"k","value":1,"extra":1}"#).is_err());
    }

    #[test]
    fn a_patch_op_parses_and_round_trips() {
        let op: Op = serde_json::from_str(r#"{"op":"patch","key":"obj/1","value":{"x":2,"gone":null},"ifRev":4}"#)
            .expect("a patch op parses");
        assert_eq!(
            op,
            Op::Patch { key: "obj/1".into(), value: serde_json::json!({"x": 2, "gone": null}), if_rev: Some(4) }
        );
        assert_eq!((op.key(), op.if_rev()), ("obj/1", Some(4)));
        assert_eq!(op.value_bytes(), r#"{"x":2,"gone":null}"#.len(), "a patch counts its partial");
        let wire = serde_json::to_value(&op).expect("serialize");
        assert_eq!(wire, serde_json::json!({"op": "patch", "key": "obj/1", "value": {"x": 2, "gone": null}, "ifRev": 4}));
        assert!(serde_json::from_str::<Op>(r#"{"op":"patch","key":"k"}"#).is_err(), "a patch needs a value");
        assert!(serde_json::from_str::<Op>(r#"{"op":"patch","key":"k","value":{},"extra":1}"#).is_err());
        assert!(serde_json::from_str::<Op>(r#"{"op":"merge","key":"k","value":{}}"#).is_err());
    }
}
