//! `~/.k2/stores/gardens.sqlite`: the Garden data backing on every
//! platform (contract §2.2, PRD §5.1). C0 → B1.
//!
//! Owned by the daemon only: mode 0600 in a 0700 dir, WAL, one connection
//! behind a mutex, opened lazily, closed after `IDLE_CLOSE_SECS` idle. Its
//! schema version lives in `_k2_meta` (not a k2so.db migration).

use std::path::{Path, PathBuf};

use super::backend::{ChangesPage, Cursor, Entry, Filter, Json, Op, Page, ReapReport, SpaceInfo, StoreBackend};
use super::{not_built, Actor, DataError, Space};

/// The file name under `~/.k2/stores/`.
pub const FILE_NAME: &str = "gardens.sqlite";

/// The schema version this build writes; a higher one is refused with
/// `store_unavailable`.
pub const SCHEMA_VERSION: u32 = 1;

/// Version 1 of the schema (contract §2.2).
pub const GARDENS_SCHEMA_V1: &str = r#"
CREATE TABLE IF NOT EXISTS _k2_meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
INSERT OR IGNORE INTO _k2_meta (k, v) VALUES ('schema_version', '1');

CREATE TABLE IF NOT EXISTS spaces (
  space        TEXT PRIMARY KEY CHECK (space LIKE 'data:%' OR space LIKE 'save:%'),
  rev          INTEGER NOT NULL DEFAULT 0,
  reaped_rev   INTEGER NOT NULL DEFAULT 0,
  bytes        INTEGER NOT NULL DEFAULT 0,
  keys         INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL,
  orphaned_at  INTEGER
);

CREATE TABLE IF NOT EXISTS entries (
  space       TEXT NOT NULL,
  key         TEXT NOT NULL,
  rev         INTEGER NOT NULL,
  deleted     INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
  value       TEXT,
  size        INTEGER NOT NULL DEFAULT 0,
  updated_at  INTEGER NOT NULL,
  updated_by  TEXT NOT NULL,
  CHECK ((deleted = 1) = (value IS NULL)),
  PRIMARY KEY (space, key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS entries_by_rev ON entries(space, rev, key);

CREATE TABLE IF NOT EXISTS changes (
  space    TEXT NOT NULL,
  rev      INTEGER NOT NULL,
  key      TEXT NOT NULL,
  deleted  INTEGER NOT NULL CHECK (deleted IN (0, 1)),
  by       TEXT NOT NULL,
  at       INTEGER NOT NULL,
  PRIMARY KEY (space, rev, key)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS changes_by_at ON changes(at);
"#;

/// The Garden data store over one SQLite file.
pub struct SqliteStore {
    path: PathBuf,
}

impl SqliteStore {
    /// A store for `<dir>/gardens.sqlite`. Nothing is opened until the
    /// first call (B1: create the dir 0700 and the file 0600, apply the
    /// schema, refuse a newer version).
    pub fn new(dir: &Path) -> SqliteStore {
        SqliteStore { path: dir.join(FILE_NAME) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Close the connection if it has been idle for `IDLE_CLOSE_SECS`.
    pub fn close_if_idle(&self) {}
}

impl StoreBackend for SqliteStore {
    fn get(&self, _space: &Space, _key: &str) -> Result<Option<Entry>, DataError> {
        Err(not_built())
    }

    fn put(&self, _space: &Space, _key: &str, _value: &Json, _if_rev: Option<i64>, _by: &Actor) -> Result<i64, DataError> {
        Err(not_built())
    }

    fn delete(&self, _space: &Space, _key: &str, _if_rev: Option<i64>, _by: &Actor) -> Result<Option<i64>, DataError> {
        Err(not_built())
    }

    fn apply(&self, _space: &Space, _ops: &[Op], _by: &Actor) -> Result<i64, DataError> {
        Err(not_built())
    }

    fn list(
        &self,
        _space: &Space,
        _prefix: &str,
        _after: Option<&str>,
        _limit: usize,
        _filter: Filter,
    ) -> Result<Page, DataError> {
        Err(not_built())
    }

    fn changes(&self, _space: &Space, _from: &Cursor, _prefix: &str, _filter: Filter) -> Result<ChangesPage, DataError> {
        Err(not_built())
    }

    fn info(&self, _space: &Space) -> Result<Option<SpaceInfo>, DataError> {
        Err(not_built())
    }

    fn spaces(&self) -> Result<Vec<SpaceInfo>, DataError> {
        Err(not_built())
    }

    fn drop_space(&self, _space: &Space) -> Result<(), DataError> {
        Err(not_built())
    }

    fn move_space(&self, _from: &Space, _to: &Space) -> Result<(), DataError> {
        Err(not_built())
    }

    fn mark_orphans(&self, _live_widgets: &[String], _now: i64) -> Result<Vec<String>, DataError> {
        Err(not_built())
    }

    fn reap(&self, _now: i64) -> Result<ReapReport, DataError> {
        Err(not_built())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema the contract freezes applies cleanly to an empty file.
    #[test]
    fn schema_v1_applies_and_names_its_version() {
        let conn = rusqlite::Connection::open_in_memory().expect("memory db");
        conn.execute_batch(GARDENS_SCHEMA_V1).expect("schema applies");
        conn.execute_batch(GARDENS_SCHEMA_V1).expect("schema is idempotent");
        let v: String = conn
            .query_row("SELECT v FROM _k2_meta WHERE k = 'schema_version'", [], |r| r.get(0))
            .expect("version row");
        assert_eq!(v, SCHEMA_VERSION.to_string());
        let bad = conn.execute(
            "INSERT INTO entries (space, key, rev, deleted, value, updated_at, updated_by) VALUES ('data:b', 'k', 1, 1, '1', 1, 'owner')",
            [],
        );
        assert!(bad.is_err(), "a tombstone has no value");
    }
}
