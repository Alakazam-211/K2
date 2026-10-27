//! `~/.k2/stores/gardens.sqlite`: the Garden data backing on every
//! platform (contract §2.2, PRD §5.1). Owner: B1.
//!
//! Owned by the daemon only: mode 0600 in a 0700 dir, WAL, one connection
//! behind a mutex, opened lazily, closed after `IDLE_CLOSE_SECS` idle. Its
//! schema version lives in `_k2_meta` (not a k2so.db migration); a newer
//! version than this build knows is refused with `store_unavailable`.
//!
//! Every write runs in one `BEGIN IMMEDIATE`: `ifRev` checks, limits, one
//! new space rev for the whole set of ops, one `changes` row per op. The
//! change feed reads `entries` (each row carries the rev that last touched
//! it; deletes leave tombstones until the reaper removes them and raises
//! `reaped_rev`).

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::backend::{
    Change, ChangesPage, Cursor, Entry, Filter, Json, Op, Page, ReapReport, SpaceInfo, StoreBackend,
};
use super::{
    keys, Actor, DataError, Space, CHANGES_PAGE_ITEMS, CHANGE_LOG_KEEP_SECS, IDLE_CLOSE_SECS, MAX_FILE_BYTES,
    MAX_SAVE_BYTES, MAX_SAVE_KEYS, MAX_SPACE_BYTES, MAX_SPACE_KEYS, MAX_VALUE_BYTES, PAGE_BYTES, TOMBSTONE_KEEP_SECS,
};

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

/// Tables added beside version 1 (prd-garden-board-needs-v1 GB6, V6). The
/// schema version stays 1, so an older K2 still opens the file and simply
/// ignores these; `blob_store::reconcile_aux` repairs them after such a K2
/// wrote.
pub const GARDENS_AUX_V1: &str = r#"
CREATE TABLE IF NOT EXISTS blobs (
  hash        TEXT PRIMARY KEY,
  size        INTEGER NOT NULL,
  mime        TEXT NOT NULL,
  created_at  INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS space_blobs (
  space         TEXT NOT NULL,
  hash          TEXT NOT NULL,
  size          INTEGER NOT NULL,
  added_at      INTEGER NOT NULL,
  added_by      TEXT NOT NULL,
  name          TEXT,
  mime          TEXT NOT NULL DEFAULT 'application/octet-stream',
  refs          INTEGER NOT NULL DEFAULT 0,
  first_ref_at  INTEGER,
  unref_at      INTEGER,
  PRIMARY KEY (space, hash)
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS blob_refs (
  space  TEXT NOT NULL,
  key    TEXT NOT NULL,
  hash   TEXT NOT NULL,
  PRIMARY KEY (space, key, hash)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS blob_refs_by_hash ON blob_refs(space, hash);
"#;

/// Every table keyed by space: a space delete or move carries all of them
/// (GB8, V8).
pub const SPACE_TABLES: [&str; 5] = ["entries", "changes", "blob_refs", "space_blobs", "spaces"];

/// Seconds since the Unix epoch (injectable for tests).
pub type Clock = Arc<dyn Fn() -> i64 + Send + Sync>;

fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    })
}

struct Open {
    conn: Connection,
    last_used: Instant,
}

/// What one op of a write does to its key, once planned.
enum Act {
    /// Store this canonical JSON text.
    Write(String),
    /// Tombstone the key (nothing when it's already gone).
    Delete,
    /// A patch op before its merge, which runs inside the write.
    Patch,
    /// A patch that changes nothing: no row, no change-log entry.
    Skip,
}

/// The Garden data store over one SQLite file.
pub struct SqliteStore {
    path: PathBuf,
    clock: Clock,
    max_file_bytes: u64,
    /// The whole blob folder (GB10; tests lower it).
    pub(super) blob_store_cap: i64,
    /// One space's held and pending blobs (GB11; tests lower it).
    pub(super) space_blob_cap: i64,
    /// Pending uploads per uploader across spaces (review #5; tests lower it).
    pub(super) pending_blob_cap: i64,
    open: Mutex<Option<Open>>,
}

/// Canonical JSON text of a value, or `value_too_large`.
pub fn canonical(value: &Json) -> Result<String, DataError> {
    let text = serde_json::to_string(value).map_err(|e| DataError::BadRequest(format!("value isn't JSON: {e}")))?;
    if text.len() > MAX_VALUE_BYTES {
        return Err(DataError::ValueTooLarge { bytes: text.len() });
    }
    Ok(text)
}

fn limits(space: &Space) -> (i64, i64) {
    if space.is_data() {
        (MAX_SPACE_BYTES, MAX_SPACE_KEYS)
    } else {
        (MAX_SAVE_BYTES, MAX_SAVE_KEYS)
    }
}

fn passes(key: &str, prefix: &str, filter: Filter) -> bool {
    keys::under(prefix, key) && filter.is_none_or(|ps| ps.iter().any(|p| keys::under(p, key)))
}

/// Rough wire bytes of one row (for page cuts).
fn row_bytes(key: &str, value_len: usize) -> usize {
    key.len() + value_len + 64
}

fn parse_value(text: Option<String>) -> Result<Json, DataError> {
    match text {
        None => Ok(Json::Null),
        Some(t) => serde_json::from_str(&t).map_err(|e| DataError::Unavailable(format!("a stored value is unreadable: {e}"))),
    }
}

impl SqliteStore {
    /// A store for `<dir>/gardens.sqlite`. Nothing is opened until the
    /// first call.
    pub fn new(dir: &Path) -> SqliteStore {
        SqliteStore::with_clock(dir, system_clock())
    }

    /// The same with an injected clock (tests).
    pub fn with_clock(dir: &Path, clock: Clock) -> SqliteStore {
        SqliteStore {
            path: dir.join(FILE_NAME),
            clock,
            max_file_bytes: MAX_FILE_BYTES,
            blob_store_cap: super::blobs::MAX_BLOB_STORE_BYTES,
            space_blob_cap: super::blobs::MAX_SPACE_BLOB_BYTES,
            pending_blob_cap: super::blobs::MAX_PENDING_PER_UPLOADER,
            open: Mutex::new(None),
        }
    }

    /// Lower the whole-file cap (tests of `store_full`).
    pub fn with_max_file_bytes(mut self, bytes: u64) -> SqliteStore {
        self.max_file_bytes = bytes;
        self
    }

    /// Lower the blob caps (tests of `space_full` / `store_full` / GC
    /// pressure): one space's blobs, and the whole folder.
    pub fn with_blob_caps(mut self, space: i64, store: i64) -> SqliteStore {
        self.space_blob_cap = space;
        self.blob_store_cap = store;
        self
    }

    /// Lower the per-uploader pending cap (tests).
    pub fn with_pending_cap(mut self, bytes: i64) -> SqliteStore {
        self.pending_blob_cap = bytes;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `<stores>/blobs/` (GB5).
    pub fn blob_dir(&self) -> PathBuf {
        self.path.parent().map(|d| d.join("blobs")).unwrap_or_else(|| PathBuf::from("blobs"))
    }

    pub fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Close the connection if it has been idle for `IDLE_CLOSE_SECS`.
    pub fn close_if_idle(&self) {
        let mut g = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if g.as_ref().is_some_and(|o| o.last_used.elapsed().as_secs() >= IDLE_CLOSE_SECS) {
            *g = None;
        }
    }

    /// Writes to `space` since `since` (seconds) from its change log, kept
    /// [`CHANGE_LOG_KEEP_SECS`] (7 days): `(by, all)`, where `by` counts the
    /// ops stored by `by` (an [`Actor::stored`] string) and `all` every op
    /// from anyone (room-scope R16, `widget.self`).
    pub fn writes_since(&self, space: &Space, by: &str, since: i64) -> Result<(i64, i64), DataError> {
        self.with(|conn| {
            Ok(conn.query_row(
                "SELECT COALESCE(SUM(by = ?2), 0), COUNT(*) FROM changes WHERE space = ?1 AND at >= ?3",
                params![space.as_str(), by, since],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
    }

    /// True while a connection is open (tests of idle close).
    pub fn is_open(&self) -> bool {
        self.open.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    fn open_conn(&self) -> Result<Connection, DataError> {
        let dir = self.path.parent().ok_or_else(|| DataError::Unavailable("the store has no folder".into()))?;
        std::fs::create_dir_all(dir).map_err(|e| DataError::Unavailable(format!("can't create the store folder: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| DataError::Unavailable(format!("can't protect the store folder: {e}")))?;
        }
        let mut conn = Connection::open(&self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| DataError::Unavailable(format!("can't protect the store file: {e}")))?;
        }
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(()))?;
        conn.execute_batch("PRAGMA synchronous = NORMAL;")?;
        let has_meta: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = '_k2_meta'",
            [],
            |r| r.get(0),
        )?;
        if has_meta {
            let v: Option<String> =
                conn.query_row("SELECT v FROM _k2_meta WHERE k = 'schema_version'", [], |r| r.get(0)).optional()?;
            let v: u32 = v.as_deref().unwrap_or("0").parse().unwrap_or(u32::MAX);
            if v > SCHEMA_VERSION {
                return Err(DataError::Unavailable(format!(
                    "Garden data was written by a newer K2 (schema {v}); update K2 to read it"
                )));
            }
        }
        conn.execute_batch(GARDENS_SCHEMA_V1)?;
        conn.execute_batch(GARDENS_AUX_V1)?;
        super::blob_store::upgrade_aux(&conn)?;
        super::blob_store::reconcile_aux(&mut conn, self.now())?;
        Ok(conn)
    }

    /// Run `f` on the open connection (opening it first).
    pub(super) fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T, DataError>) -> Result<T, DataError> {
        let mut g = self.open.lock().unwrap_or_else(|e| e.into_inner());
        if g.is_none() {
            *g = Some(Open { conn: self.open_conn()?, last_used: Instant::now() });
        }
        let o = g.as_mut().expect("opened above");
        o.last_used = Instant::now();
        f(&mut o.conn)
    }

    fn file_bytes(conn: &Connection) -> Result<u64, DataError> {
        let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        Ok((pages.max(0) as u64) * (size.max(0) as u64))
    }

    /// The core write: answers `(rev, changed)`. `changed = false` when every
    /// op was a delete of a missing key or a patch that changed nothing (no
    /// rev bump). `indexed` (a batch, transaction or import) names the
    /// failing op's index in a conflict or a refused patch.
    fn write(&self, space: &Space, ops: &[Op], by: &Actor, indexed: bool) -> Result<(i64, bool), DataError> {
        let at = |i: usize, op: &Op, e: DataError| -> DataError {
            if !indexed {
                return e;
            }
            match e {
                DataError::BadRequest(m) => DataError::BadRequest(format!("op {i} ('{}'): {m}", op.key())),
                other => other,
            }
        };
        // Shape first: keys, values, duplicates, patch partials. Nothing is
        // opened for a bad request.
        let mut seen = HashSet::new();
        let mut texts: Vec<Act> = Vec::with_capacity(ops.len());
        // Blob refs each written value names (GB2; `data:` spaces only, GB4).
        let blobs_on = space.is_data();
        let mut refs: Vec<Option<BTreeSet<String>>> = vec![None; ops.len()];
        for (i, op) in ops.iter().enumerate() {
            keys::validate_key(op.key())?;
            if !seen.insert(op.key().to_string()) {
                return Err(DataError::BadRequest(format!("'{}' appears twice in one write", op.key())));
            }
            if let (true, Op::Put { value, .. }) = (blobs_on, op) {
                refs[i] = Some(super::blobs::scan_refs(value).map_err(|e| at(i, op, e))?);
            }
            texts.push(match op {
                Op::Put { value, .. } => Act::Write(canonical(value)?),
                Op::Delete { .. } => Act::Delete,
                Op::Patch { value, if_rev, .. } => {
                    super::patch::check_partial(value, *if_rev).map_err(|e| at(i, op, e))?;
                    Act::Patch
                }
            });
        }
        let now = self.now();
        let who = by.stored();
        let max_file = self.max_file_bytes;
        self.with(|conn| {
            if texts.iter().any(|a| !matches!(a, Act::Delete)) && Self::file_bytes(conn)? > max_file {
                return Err(DataError::StoreFull);
            }
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let head: Option<(i64, i64, i64)> = tx
                .query_row("SELECT rev, bytes, keys FROM spaces WHERE space = ?1", [space.as_str()], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .optional()?;
            let (rev, mut bytes, mut nkeys) = head.unwrap_or((0, 0, 0));
            // Current state of each key: (live rev or 0, live size). A patch
            // reads the live value and merges it here, inside this write, so
            // the whole set still lands at one rev or not at all.
            let mut plan: Vec<(usize, i64, i64)> = Vec::with_capacity(ops.len());
            {
                let mut q = tx.prepare_cached("SELECT rev, deleted, size FROM entries WHERE space = ?1 AND key = ?2")?;
                let mut qv =
                    tx.prepare_cached("SELECT value FROM entries WHERE space = ?1 AND key = ?2 AND deleted = 0")?;
                for (i, op) in ops.iter().enumerate() {
                    let cur: Option<(i64, bool, i64)> = q
                        .query_row(params![space.as_str(), op.key()], |r| Ok((r.get(0)?, r.get::<_, i64>(1)? == 1, r.get(2)?)))
                        .optional()?;
                    let (live_rev, live_size) = match cur {
                        Some((r, false, s)) => (r, s),
                        _ => (0, 0),
                    };
                    if let Some(want) = op.if_rev() {
                        if want != live_rev {
                            return Err(DataError::Conflict {
                                key: op.key().to_string(),
                                rev: live_rev,
                                op: indexed.then_some(i),
                            });
                        }
                    }
                    if let Op::Patch { value: partial, .. } = op {
                        let live = if live_rev == 0 {
                            None
                        } else {
                            Some(parse_value(qv.query_row(params![space.as_str(), op.key()], |r| r.get(0)).optional()?)?)
                        };
                        texts[i] = match super::patch::merged(op.key(), live.as_ref(), partial).map_err(|e| at(i, op, e))? {
                            Some(doc) => {
                                if blobs_on {
                                    refs[i] = Some(super::blobs::scan_refs(&doc).map_err(|e| at(i, op, e))?);
                                }
                                Act::Write(canonical(&doc)?)
                            }
                            None => Act::Skip,
                        };
                    }
                    plan.push((i, live_rev, live_size));
                }
            }
            let mut effective = 0usize;
            for &(i, live_rev, live_size) in &plan {
                match &texts[i] {
                    Act::Write(t) => {
                        bytes += t.len() as i64 - live_size;
                        if live_rev == 0 {
                            nkeys += 1;
                        }
                        effective += 1;
                    }
                    Act::Delete if live_rev != 0 => {
                        bytes -= live_size;
                        nkeys -= 1;
                        effective += 1;
                    }
                    Act::Delete | Act::Skip | Act::Patch => {}
                }
            }
            if effective == 0 {
                return Ok((rev, false));
            }
            let (max_bytes, max_keys) = limits(space);
            if bytes > max_bytes {
                return Err(DataError::SpaceFull { what: "bytes" });
            }
            if nkeys > max_keys {
                return Err(DataError::SpaceFull { what: "keys" });
            }
            // GB3: every ref must name a blob this space holds, before
            // anything is written.
            if blobs_on {
                let named: BTreeSet<&str> = plan
                    .iter()
                    .filter(|(i, ..)| matches!(texts[*i], Act::Write(_)))
                    .filter_map(|(i, ..)| refs[*i].as_ref())
                    .flat_map(|r| r.iter().map(String::as_str))
                    .collect();
                super::blob_store::check_held(&tx, space, &named)?;
            }
            let new_rev = rev + 1;
            {
                let mut up = tx.prepare_cached(
                    "INSERT INTO entries (space, key, rev, deleted, value, size, updated_at, updated_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(space, key) DO UPDATE SET rev = excluded.rev, deleted = excluded.deleted,
                       value = excluded.value, size = excluded.size, updated_at = excluded.updated_at,
                       updated_by = excluded.updated_by",
                )?;
                let mut log = tx.prepare_cached(
                    "INSERT INTO changes (space, rev, key, deleted, by, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?;
                let empty = BTreeSet::new();
                for &(i, live_rev, _) in &plan {
                    let key = ops[i].key();
                    match &texts[i] {
                        Act::Write(t) => {
                            up.execute(params![space.as_str(), key, new_rev, 0, t, t.len() as i64, now, who])?;
                            log.execute(params![space.as_str(), new_rev, key, 0, who, now])?;
                            if blobs_on {
                                super::blob_store::set_key_refs(&tx, space, key, refs[i].as_ref().unwrap_or(&empty), now)?;
                            }
                        }
                        Act::Delete if live_rev != 0 => {
                            up.execute(params![space.as_str(), key, new_rev, 1, Option::<String>::None, 0, now, who])?;
                            log.execute(params![space.as_str(), new_rev, key, 1, who, now])?;
                            if blobs_on {
                                super::blob_store::set_key_refs(&tx, space, key, &empty, now)?;
                            }
                        }
                        Act::Delete | Act::Skip | Act::Patch => {}
                    }
                }
            }
            if blobs_on {
                super::blob_store::set_aux_rev(&tx, space, new_rev)?;
            }
            tx.execute(
                "INSERT INTO spaces (space, rev, bytes, keys, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)
                 ON CONFLICT(space) DO UPDATE SET rev = excluded.rev, bytes = excluded.bytes, keys = excluded.keys,
                   updated_at = excluded.updated_at",
                params![space.as_str(), new_rev, bytes, nkeys, now],
            )?;
            tx.commit()?;
            Ok((new_rev, true))
        })
    }
}

impl StoreBackend for SqliteStore {
    fn get(&self, space: &Space, key: &str) -> Result<Option<Entry>, DataError> {
        keys::validate_key(key)?;
        self.with(|conn| {
            let row: Option<(i64, Option<String>, i64, String)> = conn
                .query_row(
                    "SELECT rev, value, updated_at, updated_by FROM entries WHERE space = ?1 AND key = ?2 AND deleted = 0",
                    params![space.as_str(), key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            row.map(|(rev, value, updated_at, by)| {
                Ok(Entry { key: key.to_string(), rev, value: parse_value(value)?, updated_at, by })
            })
            .transpose()
        })
    }

    fn put(&self, space: &Space, key: &str, value: &Json, if_rev: Option<i64>, by: &Actor) -> Result<i64, DataError> {
        let op = Op::Put { key: key.to_string(), value: value.clone(), if_rev };
        self.write(space, std::slice::from_ref(&op), by, false).map(|(rev, _)| rev)
    }

    fn delete(&self, space: &Space, key: &str, if_rev: Option<i64>, by: &Actor) -> Result<Option<i64>, DataError> {
        let op = Op::Delete { key: key.to_string(), if_rev };
        self.write(space, std::slice::from_ref(&op), by, false).map(|(rev, changed)| changed.then_some(rev))
    }

    fn apply(&self, space: &Space, ops: &[Op], by: &Actor) -> Result<i64, DataError> {
        if ops.is_empty() {
            return Err(DataError::BadRequest("a write needs at least one op".into()));
        }
        self.write(space, ops, by, true).map(|(rev, _)| rev)
    }

    fn list(
        &self,
        space: &Space,
        prefix: &str,
        after: Option<&str>,
        limit: usize,
        filter: Filter,
    ) -> Result<Page, DataError> {
        keys::validate_prefix(prefix)?;
        let limit = limit.clamp(1, super::MAX_LIST_LIMIT);
        // Start at the later of the prefix and the cursor.
        let start = match after {
            Some(a) if a >= prefix => a.to_string(),
            _ => String::new(),
        };
        let exclusive = after.is_some_and(|a| a >= prefix);
        self.with(|conn| {
            let mut q = conn.prepare_cached(
                "SELECT key, rev, value, updated_at, updated_by FROM entries
                 WHERE space = ?1 AND deleted = 0 AND key >= ?2 AND key >= ?3 ORDER BY key",
            )?;
            let mut rows = q.query(params![space.as_str(), start, prefix])?;
            let mut items: Vec<Entry> = Vec::new();
            let mut bytes = 0usize;
            let mut more = false;
            while let Some(r) = rows.next()? {
                let key: String = r.get(0)?;
                if exclusive && key == start {
                    continue;
                }
                if !key.starts_with(prefix) {
                    break;
                }
                if !passes(&key, prefix, filter) {
                    continue;
                }
                let value: Option<String> = r.get(2)?;
                let sz = row_bytes(&key, value.as_ref().map_or(0, String::len));
                if items.len() >= limit || (!items.is_empty() && bytes + sz > PAGE_BYTES) {
                    more = true;
                    break;
                }
                bytes += sz;
                items.push(Entry { key, rev: r.get(1)?, value: parse_value(value)?, updated_at: r.get(3)?, by: r.get(4)? });
            }
            let next = if more { items.last().map(|e| e.key.clone()) } else { None };
            Ok(Page { items, next })
        })
    }

    fn changes(&self, space: &Space, from: &Cursor, prefix: &str, filter: Filter) -> Result<ChangesPage, DataError> {
        keys::validate_prefix(prefix)?;
        self.with(|conn| {
            let head: Option<(i64, i64)> = conn
                .query_row("SELECT rev, reaped_rev FROM spaces WHERE space = ?1", [space.as_str()], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .optional()?;
            let (head, reaped) = head.unwrap_or((0, 0));
            if from.since < reaped {
                return Err(DataError::Resync { reaped_rev: reaped });
            }
            let after = from.after.clone();
            let mut q = conn.prepare_cached(
                "SELECT key, rev, deleted, value, updated_at, updated_by FROM entries
                 WHERE space = ?1 AND (rev > ?2 OR (?3 IS NOT NULL AND rev = ?2 AND key > ?3))
                 ORDER BY rev, key",
            )?;
            let mut rows = q.query(params![space.as_str(), from.since, after])?;
            let mut items: Vec<Change> = Vec::new();
            let mut bytes = 0usize;
            let mut group_start = 0usize;
            let mut next: Option<Cursor> = None;
            let mut partial = false;
            while let Some(r) = rows.next()? {
                let key: String = r.get(0)?;
                if !passes(&key, prefix, filter) {
                    continue;
                }
                let rev: i64 = r.get(1)?;
                let value: Option<String> = r.get(3)?;
                let sz = row_bytes(&key, value.as_ref().map_or(0, String::len));
                if items.last().is_some_and(|l| l.rev != rev) {
                    group_start = items.len();
                }
                let full = items.len() >= CHANGES_PAGE_ITEMS || (!items.is_empty() && bytes + sz > PAGE_BYTES);
                if full {
                    let last = items.last().expect("a full page has items");
                    if last.rev != rev {
                        // The page ends exactly at a rev boundary.
                        next = Some(Cursor { since: last.rev, after: None });
                    } else if group_start > 0 {
                        // Drop the unfinished rev; the next page starts with it whole.
                        items.truncate(group_start);
                        let l = items.last().expect("earlier revs remain");
                        next = Some(Cursor { since: l.rev, after: None });
                    } else {
                        // One rev alone is over a page.
                        next = Some(Cursor { since: rev, after: Some(last.key.clone()) });
                        partial = true;
                    }
                    break;
                }
                bytes += sz;
                items.push(Change {
                    key,
                    rev,
                    deleted: r.get::<_, i64>(2)? == 1,
                    value: parse_value(value)?,
                    by: r.get(5)?,
                    at: r.get(4)?,
                });
            }
            Ok(ChangesPage { head, items, next, partial })
        })
    }

    fn info(&self, space: &Space) -> Result<Option<SpaceInfo>, DataError> {
        self.with(|conn| {
            conn.query_row(
                "SELECT space, rev, reaped_rev, bytes, keys, created_at, updated_at, orphaned_at FROM spaces WHERE space = ?1",
                [space.as_str()],
                space_info,
            )
            .optional()
            .map_err(DataError::from)
        })
    }

    fn spaces(&self) -> Result<Vec<SpaceInfo>, DataError> {
        self.with(|conn| {
            let mut q = conn.prepare(
                "SELECT space, rev, reaped_rev, bytes, keys, created_at, updated_at, orphaned_at FROM spaces ORDER BY space",
            )?;
            let rows = q.query_map([], space_info)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(DataError::from)
        })
    }

    fn drop_space(&self, space: &Space) -> Result<(), DataError> {
        let now = self.now();
        self.with(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // GB8 + security review #9: the blob rows stay as let-go rows
            // under a tombstone name for the 35-day keep, so a restore of
            // the pre-op snapshot finds its files; GC takes them after.
            super::blob_store::tombstone_space(&tx, space.as_str(), now)?;
            for t in SPACE_TABLES {
                tx.execute(&format!("DELETE FROM {t} WHERE space = ?1"), [space.as_str()])?;
            }
            super::blob_store::forget_aux_rev(&tx, space)?;
            tx.commit()?;
            Ok(())
        })
    }

    fn move_space(&self, from: &Space, to: &Space) -> Result<(), DataError> {
        if from.is_data() != to.is_data() {
            return Err(DataError::BadRequest("a space moves only to a space of the same kind".into()));
        }
        self.with(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let exists: bool =
                tx.query_row("SELECT COUNT(*) > 0 FROM spaces WHERE space = ?1", [from.as_str()], |r| r.get(0))?;
            if !exists {
                return Err(DataError::NotFound("space_not_found"));
            }
            let target_keys: i64 = tx
                .query_row("SELECT keys FROM spaces WHERE space = ?1", [to.as_str()], |r| r.get(0))
                .optional()?
                .unwrap_or(0);
            if target_keys > 0 {
                return Err(DataError::Refused {
                    code: "space_exists",
                    status: 409,
                    hint: format!("{} already has data; delete or export it first", to.widget()),
                });
            }
            for t in SPACE_TABLES {
                tx.execute(&format!("DELETE FROM {t} WHERE space = ?1"), [to.as_str()])?;
                tx.execute(&format!("UPDATE {t} SET space = ?2 WHERE space = ?1"), params![from.as_str(), to.as_str()])?;
            }
            super::blob_store::move_aux_rev(&tx, from, to)?;
            tx.execute("UPDATE spaces SET orphaned_at = NULL WHERE space = ?1", [to.as_str()])?;
            tx.commit()?;
            Ok(())
        })
    }

    fn mark_orphans(&self, live_widgets: &[String], now: i64) -> Result<Vec<String>, DataError> {
        let live: HashSet<&str> = live_widgets.iter().map(String::as_str).collect();
        self.with(|conn| {
            let tx = conn.transaction()?;
            let rows: Vec<(String, Option<i64>)> = {
                let mut q = tx.prepare("SELECT space, orphaned_at FROM spaces WHERE space LIKE 'data:%'")?;
                let r = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                r.collect::<Result<Vec<_>, _>>()?
            };
            let mut marked = Vec::new();
            for (space, orphaned) in rows {
                let widget = &space[5..];
                let is_live = live.contains(widget) || widget.starts_with("k2:");
                match (is_live, orphaned) {
                    (true, Some(_)) => {
                        tx.execute("UPDATE spaces SET orphaned_at = NULL WHERE space = ?1", [&space])?;
                    }
                    (false, None) => {
                        tx.execute("UPDATE spaces SET orphaned_at = ?2 WHERE space = ?1", params![&space, now])?;
                        marked.push(widget.to_string());
                    }
                    _ => {}
                }
            }
            tx.commit()?;
            Ok(marked)
        })
    }

    fn reap(&self, now: i64) -> Result<ReapReport, DataError> {
        self.with(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let cutoff = now - TOMBSTONE_KEEP_SECS;
            let tops: Vec<(String, i64)> = {
                let mut q = tx.prepare(
                    "SELECT space, MAX(rev) FROM entries WHERE deleted = 1 AND updated_at < ?1 GROUP BY space",
                )?;
                let r = q.query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?;
                r.collect::<Result<Vec<_>, _>>()?
            };
            for (space, top) in &tops {
                tx.execute(
                    "UPDATE spaces SET reaped_rev = MAX(reaped_rev, ?2) WHERE space = ?1",
                    params![space, top],
                )?;
            }
            let tombstones = tx.execute("DELETE FROM entries WHERE deleted = 1 AND updated_at < ?1", [cutoff])?;
            let log_rows = tx.execute("DELETE FROM changes WHERE at < ?1", [now - CHANGE_LOG_KEEP_SECS])?;
            tx.commit()?;
            Ok(ReapReport { tombstones, log_rows })
        })
    }
}

fn space_info(r: &rusqlite::Row) -> rusqlite::Result<SpaceInfo> {
    let space: String = r.get(0)?;
    Ok(SpaceInfo {
        widget: space.get(5..).unwrap_or("").to_string(),
        space,
        rev: r.get(1)?,
        reaped_rev: r.get(2)?,
        bytes: r.get(3)?,
        keys: r.get(4)?,
        created_at: r.get(5)?,
        updated_at: r.get(6)?,
        orphaned_at: r.get(7)?,
    })
}

#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;
