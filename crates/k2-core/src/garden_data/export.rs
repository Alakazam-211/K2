//! Export and import (contract §4.5; PRD §5.8). C0 → B3.
//!
//! Export is JSONL: one header line `{space, rev, exportedAt, snapshot}`,
//! then `{key, rev, value, updatedAt, by}` per key, filtered by the
//! caller's current read prefixes. Import (person only) is one bulk write
//! at one new rev, `merge` or `replace`.
//!
//! [`export_page`] reads the store file directly with its own read-only
//! connection inside one read transaction, so each page is consistent; a
//! snapshot file is never written. The live store's writes stay with the
//! backend (B1's single writer).

use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use super::backend::{Filter, Json, Op};
use super::keys::validate_key;
use super::{DataError, Space, MAX_EXPORT_PAGE_BYTES, MAX_EXPORT_PAGE_ITEMS, MAX_IMPORT_ITEMS, MAX_VALUE_BYTES};

/// The JSONL header line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportHeader {
    pub space: String,
    pub rev: i64,
    pub exported_at: i64,
    /// `YYYY-MM-DD` when read from a snapshot.
    pub snapshot: Option<String>,
}

/// One JSONL row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportRow {
    pub key: String,
    pub rev: i64,
    pub value: Json,
    pub updated_at: i64,
    pub by: String,
}

/// One export page.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportPage {
    pub header: ExportHeader,
    pub items: Vec<ExportRow>,
    pub next: Option<String>,
}

/// How an import treats keys it doesn't name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    Merge,
    Replace,
}

/// One import item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportItem {
    pub key: String,
    pub value: Json,
}

/// True when `filter` lets `key` through (None = everything).
pub fn passes(filter: Filter, key: &str) -> bool {
    match filter {
        None => true,
        Some(prefixes) => prefixes.iter().any(|p| key.starts_with(p.as_str())),
    }
}

/// Rows read per query while filling a page.
const CHUNK: i64 = 1_000;
/// Per-row overhead counted toward the page bytes (key, rev, times, by).
const ROW_OVERHEAD: usize = 96;

/// Open a store file for reading. A snapshot (out of WAL) opens read-only
/// and is never written. The live file is in WAL, where a reader may need
/// to create the `-shm` beside it, so it opens read-write without create
/// and `query_only`: this connection never writes a row.
fn open(file: &Path, snapshot: bool) -> Result<Connection, DataError> {
    let flags = if snapshot { OpenFlags::SQLITE_OPEN_READ_ONLY } else { OpenFlags::SQLITE_OPEN_READ_WRITE };
    let c = Connection::open_with_flags(file, flags | OpenFlags::SQLITE_OPEN_NO_MUTEX)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    if !snapshot {
        c.pragma_update(None, "query_only", true)?;
    }
    Ok(c)
}

/// One export page from a store file (`gardens.sqlite`, or a snapshot
/// opened read-only).
pub fn export_page(
    file: &Path,
    space: &Space,
    after: Option<&str>,
    filter: Filter,
    snapshot: Option<&str>,
    now: i64,
) -> Result<ExportPage, DataError> {
    export_page_capped(file, space, after, filter, snapshot, now, MAX_EXPORT_PAGE_ITEMS, MAX_EXPORT_PAGE_BYTES)
}

/// [`export_page`] with explicit page caps (tests use small ones).
#[allow(clippy::too_many_arguments)]
pub fn export_page_capped(
    file: &Path,
    space: &Space,
    after: Option<&str>,
    filter: Filter,
    snapshot: Option<&str>,
    now: i64,
    max_items: usize,
    max_bytes: usize,
) -> Result<ExportPage, DataError> {
    let mut header = ExportHeader {
        space: space.as_str().to_string(),
        rev: 0,
        exported_at: now,
        snapshot: snapshot.map(str::to_string),
    };
    if !file.exists() {
        // Nothing written yet (a missing space reads empty, contract §4.1).
        return Ok(ExportPage { header, items: Vec::new(), next: None });
    }
    let mut c = open(file, snapshot.is_some())?;
    let tx = c.transaction()?;
    header.rev = tx
        .query_row("SELECT rev FROM spaces WHERE space = ?1", [space.as_str()], |r| r.get(0))
        .optional()?
        .unwrap_or(0);
    let mut items = Vec::new();
    let mut bytes = 0usize;
    let mut cursor = after.unwrap_or("").to_string();
    let mut next = None;
    'fill: loop {
        let mut st = tx.prepare_cached(
            "SELECT key, rev, value, updated_at, updated_by FROM entries
             WHERE space = ?1 AND deleted = 0 AND key > ?2 ORDER BY key LIMIT ?3",
        )?;
        let from = cursor.clone();
        let mut rows = st.query(rusqlite::params![space.as_str(), from, CHUNK])?;
        let mut seen = 0i64;
        while let Some(r) = rows.next()? {
            seen += 1;
            let key: String = r.get(0)?;
            cursor = key.clone();
            if !passes(filter, &key) {
                continue;
            }
            if items.len() >= max_items {
                next = items.last().map(|i: &ExportRow| i.key.clone());
                break 'fill;
            }
            let text: String = r.get(2)?;
            let cost = text.len() + key.len() + ROW_OVERHEAD;
            if !items.is_empty() && bytes + cost > max_bytes {
                next = items.last().map(|i: &ExportRow| i.key.clone());
                break 'fill;
            }
            let value: Json = serde_json::from_str(&text)
                .map_err(|_| DataError::Unavailable(format!("Garden data: a stored value in {space} isn't JSON")))?;
            bytes += cost;
            items.push(ExportRow { key, rev: r.get(1)?, value, updated_at: r.get(3)?, by: r.get(4)? });
        }
        if seen < CHUNK {
            break;
        }
    }
    drop(tx);
    Ok(ExportPage { header, items, next })
}

/// Every live key of `space` in a store file (import's replace set).
pub fn live_keys(file: &Path, space: &Space) -> Result<Vec<String>, DataError> {
    if !file.exists() {
        return Ok(Vec::new());
    }
    let c = open(file, false)?;
    let mut st = c.prepare("SELECT key FROM entries WHERE space = ?1 AND deleted = 0 ORDER BY key")?;
    let keys = st.query_map([space.as_str()], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
    Ok(keys)
}

/// The ops an import applies (replace also deletes keys not named).
/// `existing` = the live keys today. Puts come in item order, then the
/// deletes in key order. Every key is checked; a key twice is refused.
pub fn import_plan(items: &[ImportItem], mode: ImportMode, existing: &[String]) -> Result<Vec<Op>, DataError> {
    if items.len() > MAX_IMPORT_ITEMS {
        return Err(DataError::TooLarge(format!("an import is at most {MAX_IMPORT_ITEMS} items")));
    }
    let mut named = BTreeSet::new();
    let mut ops = Vec::with_capacity(items.len());
    for it in items {
        validate_key(&it.key)?;
        if !named.insert(it.key.as_str()) {
            return Err(DataError::BadRequest(format!("'{}' is in the import twice", it.key)));
        }
        let bytes = serde_json::to_string(&it.value).map(|s| s.len()).unwrap_or(usize::MAX);
        if bytes > MAX_VALUE_BYTES {
            return Err(DataError::ValueTooLarge { bytes });
        }
        ops.push(Op::Put { key: it.key.clone(), value: it.value.clone(), if_rev: None });
    }
    if mode == ImportMode::Replace {
        let mut gone: Vec<&String> = existing.iter().filter(|k| !named.contains(k.as_str())).collect();
        gone.sort();
        gone.dedup();
        ops.extend(gone.into_iter().map(|k| Op::Delete { key: k.clone(), if_rev: None }));
    }
    Ok(ops)
}

/// Parse an export file (JSONL) into import items: the header line (it
/// has `space` and no `key`) is skipped; blank lines are skipped; every
/// other line needs `key` and `value` (other fields are ignored).
pub fn parse_jsonl(text: &str) -> Result<Vec<ImportItem>, DataError> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: Json = serde_json::from_str(line)
            .map_err(|_| DataError::BadRequest(format!("line {} isn't JSON", i + 1)))?;
        let Some(obj) = v.as_object() else {
            return Err(DataError::BadRequest(format!("line {} isn't a JSON object", i + 1)));
        };
        if obj.contains_key("space") && !obj.contains_key("key") {
            continue;
        }
        let (Some(Json::String(key)), Some(value)) = (obj.get("key"), obj.get("value")) else {
            return Err(DataError::BadRequest(format!("line {} needs \"key\" and \"value\"", i + 1)));
        };
        out.push(ImportItem { key: key.clone(), value: value.clone() });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::garden_data::sqlite::GARDENS_SCHEMA_V1;
    use serde_json::json;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        let d = std::env::temp_dir().join(format!("k2-gd-export-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).expect("temp dir");
        d
    }

    /// A store with `data:board` at rev 9: live keys, a tombstone, and a
    /// second space that must never leak.
    fn store(dir: &Path) -> PathBuf {
        let p = dir.join("gardens.sqlite");
        let c = Connection::open(&p).expect("store");
        c.execute_batch(GARDENS_SCHEMA_V1).expect("schema");
        c.execute_batch(
            "INSERT INTO spaces (space, rev, bytes, keys, created_at, updated_at) VALUES ('data:board', 9, 30, 4, 1, 9);
             INSERT INTO spaces (space, rev, bytes, keys, created_at, updated_at) VALUES ('data:other', 2, 3, 1, 1, 2);
             INSERT INTO entries VALUES ('data:board', 'obj/1', 3, 0, '{\"x\":1}', 7, 3, 'owner');
             INSERT INTO entries VALUES ('data:board', 'obj/2', 9, 0, '\"two\"', 5, 9, 'agent:w-1');
             INSERT INTO entries VALUES ('data:board', 'obj/3', 8, 1, NULL, 0, 8, 'owner');
             INSERT INTO entries VALUES ('data:board', 'project/7/obj/9', 5, 0, '[1,2]', 5, 5, 'widget:board');
             INSERT INTO entries VALUES ('data:board', 'z', 6, 0, 'null', 4, 6, 'owner');
             INSERT INTO entries VALUES ('data:other', 'obj/1', 2, 0, '1', 1, 2, 'owner');",
        )
        .expect("rows");
        p
    }

    #[test]
    fn export_reads_live_keys_in_order_without_tombstones() {
        let d = tmp();
        let f = store(&d);
        let space = Space::data("board").expect("space");
        let page = export_page(&f, &space, None, None, None, 100).expect("page");
        assert_eq!(page.header, ExportHeader { space: "data:board".into(), rev: 9, exported_at: 100, snapshot: None });
        let keys: Vec<&str> = page.items.iter().map(|i| i.key.as_str()).collect();
        assert_eq!(keys, ["obj/1", "obj/2", "project/7/obj/9", "z"]);
        assert_eq!(page.items[0].value, json!({"x": 1}));
        assert_eq!(page.items[1].by, "agent:w-1");
        assert_eq!(page.items[3].value, Json::Null);
        assert_eq!(page.next, None);
        // A space never written reads empty, rev 0.
        let empty = export_page(&f, &Space::data("nothing").expect("space"), None, None, None, 1).expect("empty");
        assert_eq!((empty.header.rev, empty.items.len(), empty.next), (0, 0, None));
        // No store file at all: the same.
        let none = export_page(&d.join("missing.sqlite"), &space, None, None, None, 1).expect("no file");
        assert_eq!((none.header.rev, none.items.len()), (0, 0));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn export_filters_inside_the_paging_loop() {
        let d = tmp();
        let f = store(&d);
        let space = Space::data("board").expect("space");
        let prefixes = vec!["project/7/".to_string()];
        let page = export_page(&f, &space, None, Some(&prefixes), Some("2026-10-13"), 5).expect("page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].key, "project/7/obj/9");
        assert_eq!(page.header.snapshot.as_deref(), Some("2026-10-13"));
        // An empty filter list sees nothing.
        let none: Vec<String> = Vec::new();
        assert!(export_page(&f, &space, None, Some(&none), None, 5).expect("page").items.is_empty());
        // Paging by count: 1 per page walks every key exactly once.
        let mut after: Option<String> = None;
        let mut got = Vec::new();
        for _ in 0..10 {
            let p = export_page_capped(&f, &space, after.as_deref(), None, None, 1, 1, usize::MAX).expect("page");
            got.extend(p.items.iter().map(|i| i.key.clone()));
            after = p.next;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(got, ["obj/1", "obj/2", "project/7/obj/9", "z"]);
        // Paging by bytes: a page always takes one row, then stops at the cap.
        let p = export_page_capped(&f, &space, None, None, None, 1, 100, 1).expect("page");
        assert_eq!((p.items.len(), p.next.as_deref()), (1, Some("obj/1")));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn export_pages_across_query_chunks_with_a_filter() {
        let d = tmp();
        let p = d.join("gardens.sqlite");
        let c = Connection::open(&p).expect("store");
        c.execute_batch(GARDENS_SCHEMA_V1).expect("schema");
        c.execute("INSERT INTO spaces (space, rev, created_at, updated_at) VALUES ('data:b', 1, 1, 1)", []).expect("sp");
        for i in 0..2_500 {
            let key = if i % 2 == 0 { format!("a/{i:05}") } else { format!("b/{i:05}") };
            c.execute(
                "INSERT INTO entries VALUES ('data:b', ?1, 1, 0, '1', 1, 1, 'owner')",
                [key],
            )
            .expect("row");
        }
        drop(c);
        let space = Space::data("b").expect("space");
        let only_b = vec!["b/".to_string()];
        let page = export_page_capped(&p, &space, None, Some(&only_b), None, 1, 400, usize::MAX).expect("page");
        assert_eq!(page.items.len(), 400);
        assert!(page.items.iter().all(|i| i.key.starts_with("b/")));
        let page2 = export_page_capped(&p, &space, page.next.as_deref(), Some(&only_b), None, 1, 5_000, usize::MAX)
            .expect("rest");
        assert_eq!(page2.items.len(), 850);
        assert_eq!(page2.next, None);
        std::fs::remove_dir_all(&d).expect("cleanup");
    }

    #[test]
    fn import_plan_merge_and_replace() {
        let items = vec![
            ImportItem { key: "obj/1".into(), value: json!(1) },
            ImportItem { key: "obj/4".into(), value: json!({"a": 2}) },
        ];
        let existing = vec!["obj/1".to_string(), "obj/2".to_string(), "obj/0".to_string()];
        let merge = import_plan(&items, ImportMode::Merge, &existing).expect("merge");
        assert_eq!(merge.len(), 2);
        assert!(merge.iter().all(|o| matches!(o, Op::Put { if_rev: None, .. })));
        let replace = import_plan(&items, ImportMode::Replace, &existing).expect("replace");
        let deletes: Vec<&str> =
            replace.iter().filter(|o| matches!(o, Op::Delete { .. })).map(|o| o.key()).collect();
        assert_eq!(deletes, ["obj/0", "obj/2"]);
        assert_eq!(replace.len(), 4);
        // An empty replace deletes everything.
        assert_eq!(import_plan(&[], ImportMode::Replace, &existing).expect("empty").len(), 3);
    }

    #[test]
    fn import_plan_refuses_bad_input() {
        let twice = vec![
            ImportItem { key: "k".into(), value: json!(1) },
            ImportItem { key: "k".into(), value: json!(2) },
        ];
        assert!(matches!(import_plan(&twice, ImportMode::Merge, &[]), Err(DataError::BadRequest(_))));
        let bad = vec![ImportItem { key: "/x".into(), value: json!(1) }];
        assert!(matches!(import_plan(&bad, ImportMode::Merge, &[]), Err(DataError::BadKey(_))));
        let big = vec![ImportItem { key: "k".into(), value: json!("x".repeat(MAX_VALUE_BYTES)) }];
        assert!(matches!(import_plan(&big, ImportMode::Merge, &[]), Err(DataError::ValueTooLarge { .. })));
        let many: Vec<ImportItem> =
            (0..=MAX_IMPORT_ITEMS).map(|i| ImportItem { key: format!("k{i}"), value: json!(1) }).collect();
        assert!(matches!(import_plan(&many, ImportMode::Merge, &[]), Err(DataError::TooLarge(_))));
    }

    #[test]
    fn jsonl_round_trips_an_export() {
        let d = tmp();
        let f = store(&d);
        let space = Space::data("board").expect("space");
        let page = export_page(&f, &space, None, None, None, 100).expect("page");
        let mut text = serde_json::to_string(&page.header).expect("header") + "\n";
        for row in &page.items {
            text += &(serde_json::to_string(row).expect("row") + "\n");
        }
        text += "\n";
        let items = parse_jsonl(&text).expect("parse");
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], ImportItem { key: "obj/1".into(), value: json!({"x": 1}) });
        assert_eq!(live_keys(&f, &space).expect("keys"), ["obj/1", "obj/2", "project/7/obj/9", "z"]);
        assert!(matches!(parse_jsonl("{\"key\":\"k\"}"), Err(DataError::BadRequest(_))));
        assert!(matches!(parse_jsonl("[1]"), Err(DataError::BadRequest(_))));
        assert!(matches!(parse_jsonl("not json"), Err(DataError::BadRequest(_))));
        std::fs::remove_dir_all(&d).expect("cleanup");
    }
}
