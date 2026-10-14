//! Export and import (contract §4.5; PRD §5.8). C0 → B3.
//!
//! Export is JSONL: one header line `{space, rev, exportedAt, snapshot}`,
//! then `{key, rev, value, updatedAt, by}` per key, filtered by the
//! caller's current read prefixes. Import (person only) is one bulk write
//! at one new rev, `merge` or `replace`.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::backend::{Filter, Json, Op};
use super::{not_built, DataError, Space};

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

/// One export page from a store file (`gardens.sqlite`, or a snapshot
/// opened read-only).
pub fn export_page(
    _file: &Path,
    _space: &Space,
    _after: Option<&str>,
    _filter: Filter,
    _snapshot: Option<&str>,
    _now: i64,
) -> Result<ExportPage, DataError> {
    Err(not_built())
}

/// The ops an import applies (replace also deletes keys not named).
/// `existing` = the live keys today.
pub fn import_plan(_items: &[ImportItem], _mode: ImportMode, _existing: &[String]) -> Result<Vec<Op>, DataError> {
    Err(not_built())
}
