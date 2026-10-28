//! Full history, snapshots and restore (prd-garden-board-needs-v1 §6,
//! GH1–GH13; vs-live V6, V7, V16, R1–R3).
//!
//! - **Before-values (GH1).** `changes` gains `prev` (the key's canonical
//!   JSON just before the change) and `prev_kind` (0: the key didn't exist;
//!   1: `prev` holds it; NULL: unknown, a row an older K2 wrote or the trim
//!   cleared), plus `restore_id` (GH9). They are added with `ALTER TABLE`
//!   beside schema version 1, which stays 1 (V6): an older K2 still opens
//!   the file, names its own columns on insert and leaves these NULL.
//! - **History (GH4, §6.2).** `store.history` reads `changes`, newest first,
//!   every change with its value before and after. `store.changes` is
//!   unchanged (the latest change per key, from `entries`) apart from the
//!   optional restore mark.
//! - **Restore (GH5, GH10, GH11).** A restore is a plan (each key back to
//!   its value at the target rev, or to a snapshot's value), then one write
//!   at one new rev whose changes carry the restore's id, with a `restores`
//!   row in the same transaction. Every op carries `ifRev` of the state the
//!   plan saw, so a write in between makes the plan stale (`conflict`) and
//!   [`SqliteStore::restore`] plans again.
//! - **Snapshots (§6.2).** `store.snapshot` writes the chosen keys at one
//!   rev as a JSON blob in the same space; it is pending until a key names
//!   it. A snapshot restore always uses the key set the snapshot recorded
//!   (R1).
//! - **Trim (GH2).** Past [`MAX_HISTORY_BYTES`] of before-values in a space,
//!   the hourly pass clears the oldest rows' `prev`; the rows stay.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde_json::json;

use super::backend::{Filter, Json, Op};
use super::sqlite::SqliteStore;
use super::{keys, Actor, DataError, Space, CHANGE_LOG_KEEP_SECS};

/// The `/boot-status` feature and catalog `feature` of these verbs.
pub const FEATURE: &str = "garden-history-v1";
/// Before-values kept per space (GH2, Q7); the oldest are trimmed past it.
pub const MAX_HISTORY_BYTES: i64 = 32 * 1024 * 1024;
/// A `store.history` page (R5: replies into the frame aren't capped).
pub const HISTORY_PAGE_BYTES: usize = 128 * 1024;
pub const MAX_HISTORY_LIMIT: usize = 200;
pub const DEFAULT_HISTORY_LIMIT: usize = 100;
/// `prefixes` and `except` on a restore or a snapshot (R3).
pub const MAX_RESTORE_PREFIXES: usize = 16;
/// A dry run lists at most this many keys.
pub const RESTORE_KEYS_LISTED: usize = 50;
/// The manifest's `restoreSkips` (GH11).
pub const MAX_RESTORE_SKIPS: usize = 16;
/// A snapshot label.
pub const MAX_LABEL_CHARS: usize = 120;
/// One restore per this many seconds per placement or agent (GH5).
pub const RESTORE_EVERY_SECS: u64 = 10;
/// Plans made again after a write landed in between (then `conflict`).
pub const RESTORE_TRIES: usize = 3;
/// A snapshot blob's `format` and `v` (`v: 1` never shipped, GH10).
pub const SNAPSHOT_FORMAT: &str = "k2-store-snapshot";
pub const SNAPSHOT_VERSION: u64 = 2;

/// Tables and indexes added beside schema version 1 (V6).
pub(super) const HISTORY_AUX: &str = r#"
CREATE TABLE IF NOT EXISTS restores (
  id        TEXT PRIMARY KEY,
  space     TEXT NOT NULL,
  by        TEXT NOT NULL,
  at        INTEGER NOT NULL,
  source    TEXT NOT NULL,
  prefixes  TEXT NOT NULL,
  excepts   TEXT NOT NULL,
  rev       INTEGER NOT NULL,
  written   INTEGER NOT NULL,
  deleted   INTEGER NOT NULL,
  families  TEXT NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS restores_by_space ON restores(space, at);
CREATE INDEX IF NOT EXISTS changes_by_key ON changes(space, key, rev);
"#;

/// Add the history columns to `changes` once (GH1, GH9), then the tables.
pub(super) fn upgrade(conn: &Connection) -> Result<(), DataError> {
    let cols: BTreeSet<String> = {
        let mut q = conn.prepare("SELECT name FROM pragma_table_info('changes')")?;
        let r = q.query_map([], |r| r.get(0))?;
        r.collect::<Result<_, _>>()?
    };
    for (col, ty) in [("prev", "TEXT"), ("prev_kind", "INTEGER"), ("restore_id", "TEXT")] {
        if !cols.contains(col) {
            conn.execute_batch(&format!("ALTER TABLE changes ADD COLUMN {col} {ty};"))?;
        }
    }
    conn.execute_batch(HISTORY_AUX)?;
    Ok(())
}

// ── key sets ──────────────────────────────────────────────────────────

/// Which keys a restore or a snapshot covers: under one of `prefixes`
/// (none = the whole space), under none of `except`, and not under a
/// manifest `restoreSkips` prefix unless a prefix names it (GH11: a
/// prefix at or inside the skip).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeySet {
    pub prefixes: Vec<String>,
    pub except: Vec<String>,
    pub skips: Vec<String>,
}

fn clean_prefixes(what: &str, list: Vec<String>, max: usize) -> Result<Vec<String>, DataError> {
    if list.len() > max {
        return Err(DataError::BadRequest(format!("{what} lists at most {max} prefixes")));
    }
    let mut out: Vec<String> = Vec::new();
    for p in list {
        if p.is_empty() {
            return Err(DataError::BadRequest(format!("{what}: a prefix isn't empty (leave {what} out for the whole space)")));
        }
        keys::validate_prefix(&p)?;
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

impl KeySet {
    pub fn new(prefixes: Vec<String>, except: Vec<String>, skips: Vec<String>) -> Result<KeySet, DataError> {
        Ok(KeySet {
            prefixes: clean_prefixes("prefixes", prefixes, MAX_RESTORE_PREFIXES)?,
            except: clean_prefixes("except", except, MAX_RESTORE_PREFIXES)?,
            skips: clean_prefixes("restoreSkips", skips, MAX_RESTORE_SKIPS)?,
        })
    }

    /// No prefixes: the whole space (GH6's person-only rule for agents).
    pub fn whole(&self) -> bool {
        self.prefixes.is_empty()
    }

    pub fn contains(&self, key: &str) -> bool {
        let inside = self.prefixes.is_empty() || self.prefixes.iter().any(|p| keys::under(p, key));
        if !inside || self.except.iter().any(|e| keys::under(e, key)) {
            return false;
        }
        // A skipped family is left alone unless a prefix names it.
        !self.skips.iter().any(|s| {
            keys::under(s, key) && !self.prefixes.iter().any(|p| p.starts_with(s.as_str()) && keys::under(p, key))
        })
    }
}

/// A key's top-level family: up to and including its first `/` (the whole
/// key when it has none). R2b: the Settings warning reads these.
pub fn family(key: &str) -> String {
    match key.find('/') {
        Some(i) => key[..=i].to_string(),
        None => key.to_string(),
    }
}

// ── history ───────────────────────────────────────────────────────────

/// What a change's `prev` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrevKind {
    /// `prev` holds the value before.
    Value,
    /// The key didn't exist before.
    None,
    /// Not kept: trimmed, or written by an older K2.
    Unknown,
}

impl PrevKind {
    fn of(stored: Option<i64>) -> PrevKind {
        match stored {
            Some(1) => PrevKind::Value,
            Some(0) => PrevKind::None,
            _ => PrevKind::Unknown,
        }
    }
}

/// One `store.history` row. `by` is stored (`Actor::stored`); the routes
/// turn it into the wire form.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    pub key: String,
    pub rev: i64,
    pub deleted: bool,
    pub value: Json,
    pub prev: Json,
    pub prev_kind: PrevKind,
    pub by: String,
    pub at: i64,
    pub restore: Option<String>,
}

/// One `store.history` page, newest first. `next`: rows strictly before
/// `(before, after)`, as [`HistoryQuery`] takes them.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryPage {
    pub head: i64,
    pub oldest: i64,
    pub items: Vec<HistoryItem>,
    pub next: Option<(i64, Option<String>)>,
}

/// What `store.history` asks for. `before` (and `after`, a key inside the
/// rev `before`) continue a page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HistoryQuery {
    pub key: Option<String>,
    pub prefix: String,
    pub before: Option<i64>,
    pub after: Option<String>,
    pub limit: usize,
}

fn parse_text(t: Option<String>) -> Result<Json, DataError> {
    match t {
        None => Ok(Json::Null),
        Some(t) => serde_json::from_str(&t).map_err(|e| DataError::Unavailable(format!("a stored value is unreadable: {e}"))),
    }
}

fn passes(key: &str, filter: Filter) -> bool {
    filter.is_none_or(|ps| ps.iter().any(|p| keys::under(p, key)))
}

/// `(head, oldest)`: the space's rev, and the oldest rev a restore can go
/// back to (GH3). Every change after it must still be in the log with a
/// known before-value: the log is whole from its first row on (the reaper
/// cuts by age, oldest first), and a row with an unknown before-value
/// can't be undone.
fn restorable(conn: &Connection, space: &Space) -> Result<(i64, i64), DataError> {
    let head: i64 = conn
        .query_row("SELECT rev FROM spaces WHERE space = ?1", [space.as_str()], |r| r.get(0))
        .optional()?
        .unwrap_or(0);
    let (min, unknown): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT MIN(rev), MAX(CASE WHEN prev_kind IS NULL THEN rev END) FROM changes WHERE space = ?1",
        [space.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let oldest = match min {
        None => head,
        Some(m) => (m - 1).max(unknown.unwrap_or(0)).max(0),
    };
    Ok((head, oldest.min(head)))
}

impl SqliteStore {
    /// `(head, oldest)` for `store.history` and the restore checks.
    pub fn restorable(&self, space: &Space) -> Result<(i64, i64), DataError> {
        self.with(|conn| restorable(conn, space))
    }

    /// One page of every change kept, newest first (§6.2). `filter`: an
    /// agent's read prefixes.
    pub fn history(&self, space: &Space, q: &HistoryQuery, filter: Filter) -> Result<HistoryPage, DataError> {
        if let Some(k) = &q.key {
            keys::validate_key(k)?;
        }
        keys::validate_prefix(&q.prefix)?;
        let limit = q.limit.clamp(1, MAX_HISTORY_LIMIT);
        self.with(|conn| {
            let (head, oldest) = restorable(conn, space)?;
            let before = q.before.unwrap_or(i64::MAX);
            let after = if q.before.is_some() { q.after.clone() } else { None };
            let mut sql = String::from(
                "SELECT key, rev, deleted, prev, prev_kind, by, at, restore_id FROM changes
                 WHERE space = ?1 AND (rev < ?2 OR (?3 IS NOT NULL AND rev = ?2 AND key > ?3))",
            );
            if q.key.is_some() {
                sql.push_str(" AND key = ?4");
            } else if !q.prefix.is_empty() {
                sql.push_str(" AND substr(key, 1, length(?4)) = ?4");
            }
            sql.push_str(" ORDER BY rev DESC, key ASC");
            let narrow: Option<&str> = q.key.as_deref().or((!q.prefix.is_empty()).then_some(q.prefix.as_str()));
            type Row = (String, i64, bool, Option<String>, Option<i64>, String, i64, Option<String>);
            let mut rows: Vec<Row> = Vec::new();
            let mut more = false;
            {
                let mut st = conn.prepare_cached(&sql)?;
                let map = |r: &rusqlite::Row| -> rusqlite::Result<Row> {
                    Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? == 1, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
                };
                let mut it = match narrow {
                    Some(n) => st.query(params![space.as_str(), before, after, n])?,
                    None => st.query(params![space.as_str(), before, after])?,
                };
                // A row counts its key and about twice its before-value
                // (the value after is about the same size).
                let mut bytes = 0usize;
                while let Some(r) = it.next()? {
                    let row = map(r)?;
                    if !passes(&row.0, filter) {
                        continue;
                    }
                    let sz = row.0.len() + 2 * row.3.as_ref().map_or(0, String::len) + 64;
                    if rows.len() >= limit || (!rows.is_empty() && bytes + sz > HISTORY_PAGE_BYTES) {
                        more = true;
                        break;
                    }
                    bytes += sz;
                    rows.push(row);
                }
            }
            // Each change's own value: the next change's before-value, or
            // the live entry when it is the latest.
            let mut next_prev = conn.prepare_cached(
                "SELECT prev, prev_kind FROM changes WHERE space = ?1 AND key = ?2 AND rev > ?3 ORDER BY rev ASC LIMIT 1",
            )?;
            let mut live =
                conn.prepare_cached("SELECT value FROM entries WHERE space = ?1 AND key = ?2 AND rev = ?3 AND deleted = 0")?;
            let mut items = Vec::with_capacity(rows.len());
            for (key, rev, deleted, prev, prev_kind, by, at, restore) in rows {
                let value = if deleted {
                    Json::Null
                } else {
                    let after: Option<(Option<String>, Option<i64>)> = next_prev
                        .query_row(params![space.as_str(), key, rev], |r| Ok((r.get(0)?, r.get(1)?)))
                        .optional()?;
                    match after {
                        Some((p, Some(1))) => parse_text(p)?,
                        Some(_) => Json::Null,
                        None => parse_text(live.query_row(params![space.as_str(), key, rev], |r| r.get(0)).optional()?)?,
                    }
                };
                let kind = PrevKind::of(prev_kind);
                let prev = if kind == PrevKind::Value { parse_text(prev)? } else { Json::Null };
                items.push(HistoryItem { key, rev, deleted, value, prev, prev_kind: kind, by, at, restore });
            }
            let next = if more {
                items.last().map(|l| (l.rev, Some(l.key.clone())))
            } else {
                None
            };
            Ok(HistoryPage { head, oldest, items, next })
        })
    }
}

// ── restore ───────────────────────────────────────────────────────────

/// Where a restore goes back to.
#[derive(Debug, Clone, PartialEq)]
pub enum RestoreTarget {
    /// The state right after this rev.
    Rev(i64),
    /// The state at this time (seconds): the last rev at or before it.
    At(i64),
    /// A snapshot's items (read and parsed by the caller).
    Snapshot(SnapshotDoc),
}

/// A parsed snapshot blob (§6.2, `v: 2`).
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotDoc {
    pub hash: String,
    pub widget: String,
    pub prefixes: Vec<String>,
    pub except: Vec<String>,
    pub rev: i64,
    pub items: BTreeMap<String, Json>,
}

/// Read a snapshot blob's bytes; `bad_args`-shaped refusals when it isn't
/// one, or belongs to another widget.
pub fn parse_snapshot(hash: &str, bytes: &[u8], space: &Space) -> Result<SnapshotDoc, DataError> {
    let not = |why: &str| DataError::BadRequest(format!("that ref isn't a store snapshot ({why})"));
    let v: Json = serde_json::from_slice(bytes).map_err(|_| not("not JSON"))?;
    if v.get("format").and_then(Json::as_str) != Some(SNAPSHOT_FORMAT) {
        return Err(not("no k2-store-snapshot format"));
    }
    if v.get("v").and_then(Json::as_u64) != Some(SNAPSHOT_VERSION) {
        return Err(not("an unknown version"));
    }
    let widget = v.get("widget").and_then(Json::as_str).ok_or_else(|| not("no widget"))?.to_string();
    if widget != space.widget() {
        return Err(DataError::BadRequest(format!("that snapshot is {widget}'s data, not {}'s", space.widget())));
    }
    let list = |k: &str| -> Result<Vec<String>, DataError> {
        v.get(k)
            .and_then(Json::as_array)
            .ok_or_else(|| not(k))?
            .iter()
            .map(|p| p.as_str().map(str::to_string).ok_or_else(|| not(k)))
            .collect()
    };
    let prefixes = list("prefixes")?;
    let except = list("except")?;
    let rev = v.get("rev").and_then(Json::as_i64).ok_or_else(|| not("no rev"))?;
    let mut items = BTreeMap::new();
    for it in v.get("items").and_then(Json::as_array).ok_or_else(|| not("no items"))? {
        let key = it.get("key").and_then(Json::as_str).ok_or_else(|| not("an item has no key"))?;
        keys::validate_key(key)?;
        let value = it.get("value").cloned().ok_or_else(|| not("an item has no value"))?;
        items.insert(key.to_string(), value);
    }
    Ok(SnapshotDoc { hash: hash.to_string(), widget, prefixes, except, rev, items })
}

/// What a restore will do, once planned against the live state.
#[derive(Debug, Clone, PartialEq)]
pub struct RestorePlan {
    pub ops: Vec<Op>,
    /// The rev it goes back to (`None` for a snapshot).
    pub to_rev: Option<i64>,
    pub written: usize,
    pub deleted: usize,
    /// Top-level families it rewrites (R2b).
    pub families: Vec<String>,
    /// The first [`RESTORE_KEYS_LISTED`] keys it rewrites.
    pub keys: Vec<String>,
    /// The key set it covered (a snapshot's own, R1).
    pub set: KeySet,
    /// `{toRev}`, `{at, toRev}` or `{snapshot}` for the `restores` row.
    pub source: Json,
}

/// A finished restore: its id and new rev (both `None` when nothing
/// changed), with the plan's counts.
#[derive(Debug, Clone, PartialEq)]
pub struct Restored {
    pub id: Option<String>,
    pub rev: Option<i64>,
    pub plan: RestorePlan,
}

/// The mark a restore's write carries (GH9).
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RestoreMark {
    pub id: String,
    pub source: Json,
    pub prefixes: Vec<String>,
    pub except: Vec<String>,
    pub written: usize,
    pub deleted: usize,
    pub families: Vec<String>,
}

/// Inside a restore's write: one `restores` row (GH9).
pub(super) fn record_restore(tx: &Transaction, space: &Space, by: &str, at: i64, rev: i64, m: &RestoreMark) -> Result<(), DataError> {
    tx.execute(
        "INSERT INTO restores (id, space, by, at, source, prefixes, excepts, rev, written, deleted, families)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            m.id,
            space.as_str(),
            by,
            at,
            m.source.to_string(),
            json!(m.prefixes).to_string(),
            json!(m.except).to_string(),
            rev,
            m.written as i64,
            m.deleted as i64,
            json!(m.families).to_string(),
        ],
    )?;
    Ok(())
}

fn no_history() -> DataError {
    DataError::Refused {
        code: "no_history",
        status: 409,
        hint: "older than kept history (7 days of changes); the person can export a daily snapshot with k2 zen data export --at".into(),
    }
}

/// The rev a time means: the last change at or before it. Before the
/// first change kept, only a log that starts at rev 1 knows (the space was
/// empty); otherwise the reaper took what happened then.
fn rev_at(conn: &Connection, space: &Space, t: i64) -> Result<i64, DataError> {
    let last: Option<i64> =
        conn.query_row("SELECT MAX(rev) FROM changes WHERE space = ?1 AND at <= ?2", params![space.as_str(), t], |r| r.get(0))?;
    if let Some(r) = last {
        return Ok(r);
    }
    let (min, updated): (Option<i64>, Option<i64>) = conn.query_row(
        "SELECT (SELECT MIN(rev) FROM changes WHERE space = ?1), (SELECT updated_at FROM spaces WHERE space = ?1)",
        [space.as_str()],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    match (min, updated) {
        (Some(1), _) | (None, None) => Ok(0),
        (None, Some(u)) if u <= t => {
            Ok(conn.query_row("SELECT rev FROM spaces WHERE space = ?1", [space.as_str()], |r| r.get(0))?)
        }
        _ => Err(no_history()),
    }
}

/// The live `(rev, canonical text)` of every key in the space the set
/// covers, or of the named keys.
fn live_rows(conn: &Connection, space: &Space) -> Result<BTreeMap<String, (i64, String)>, DataError> {
    let mut q = conn.prepare_cached("SELECT key, rev, value FROM entries WHERE space = ?1 AND deleted = 0")?;
    let r = q.query_map([space.as_str()], |r| Ok((r.get::<_, String>(0)?, (r.get::<_, i64>(1)?, r.get::<_, String>(2)?))))?;
    r.collect::<Result<BTreeMap<_, _>, _>>().map_err(DataError::from)
}

/// Ops that bring every key in `targets` to its target (`None`: absent),
/// each with `ifRev` of the live state seen now.
fn plan_ops(
    live: &BTreeMap<String, (i64, String)>,
    targets: BTreeMap<String, Option<String>>,
    can_write: &dyn Fn(&str) -> bool,
) -> Result<(Vec<Op>, usize, usize, Vec<String>, Vec<String>), DataError> {
    let mut ops = Vec::new();
    let (mut written, mut deleted) = (0usize, 0usize);
    let mut families = BTreeSet::new();
    let mut listed = Vec::new();
    for (key, want) in targets {
        let cur = live.get(&key);
        let op = match (&want, cur) {
            (None, None) => None,
            (None, Some((rev, _))) => Some(Op::Delete { key: key.clone(), if_rev: Some(*rev) }),
            (Some(text), Some((_, now))) if text == now => None,
            (Some(text), cur) => {
                let value: Json = serde_json::from_str(text)
                    .map_err(|e| DataError::Unavailable(format!("a kept value is unreadable: {e}")))?;
                Some(Op::Put { key: key.clone(), value, if_rev: Some(cur.map_or(0, |c| c.0)) })
            }
        };
        let Some(op) = op else { continue };
        if !can_write(&key) {
            return Err(DataError::NotGranted { key: None });
        }
        match op {
            Op::Delete { .. } => deleted += 1,
            _ => written += 1,
        }
        families.insert(family(&key));
        if listed.len() < RESTORE_KEYS_LISTED {
            listed.push(key);
        }
        ops.push(op);
    }
    Ok((ops, written, deleted, families.into_iter().collect(), listed))
}

impl SqliteStore {
    /// Plan a restore against the live state now (nothing is written).
    /// `can_write`: the caller's write rights per key; a plan that touches a
    /// key outside them is `not_granted` (whole, never half).
    pub fn restore_plan(
        &self,
        space: &Space,
        target: &RestoreTarget,
        set: &KeySet,
        can_write: &dyn Fn(&str) -> bool,
    ) -> Result<RestorePlan, DataError> {
        if !space.is_data() {
            return Err(DataError::BadRequest("only a widget's shared data (data:) is restored".into()));
        }
        self.with(|conn| {
            let live = live_rows(conn, space)?;
            match target {
                RestoreTarget::Snapshot(doc) => {
                    // R1: exactly the keys the snapshot saved (its recorded
                    // prefixes and except), minus restoreSkips.
                    let snap_set = KeySet::new(doc.prefixes.clone(), doc.except.clone(), set.skips.clone())?;
                    let named = !set.prefixes.is_empty() || !set.except.is_empty();
                    if named && (set.prefixes != snap_set.prefixes || set.except != snap_set.except) {
                        let saved = if snap_set.prefixes.is_empty() { "the whole space".to_string() } else { snap_set.prefixes.join(", ") };
                        return Err(DataError::BadRequest(format!(
                            "a snapshot restores the keys it saved ({saved}); leave prefixes and except out"
                        )));
                    }
                    let mut targets: BTreeMap<String, Option<String>> = BTreeMap::new();
                    for k in live.keys().filter(|k| snap_set.contains(k)) {
                        targets.insert(k.clone(), None);
                    }
                    for (k, v) in doc.items.iter().filter(|(k, _)| snap_set.contains(k)) {
                        let text = super::sqlite::canonical(v)?;
                        targets.insert(k.clone(), Some(text));
                    }
                    let (ops, written, deleted, families, listed) = plan_ops(&live, targets, can_write)?;
                    Ok(RestorePlan {
                        ops,
                        to_rev: None,
                        written,
                        deleted,
                        families,
                        keys: listed,
                        set: snap_set,
                        source: json!({ "snapshot": super::blobs::ref_of(&doc.hash), "rev": doc.rev }),
                    })
                }
                RestoreTarget::Rev(_) | RestoreTarget::At(_) => {
                    let (head, oldest) = restorable(conn, space)?;
                    let (to_rev, source) = match target {
                        RestoreTarget::Rev(r) => (*r, json!({ "toRev": r })),
                        RestoreTarget::At(t) => {
                            let r = rev_at(conn, space, *t)?;
                            (r, json!({ "at": t, "toRev": r }))
                        }
                        RestoreTarget::Snapshot(_) => unreachable!("matched above"),
                    };
                    if to_rev > head {
                        return Err(DataError::BadRequest(format!("toRev {to_rev} is past this widget's data (rev {head})")));
                    }
                    if to_rev < oldest {
                        return Err(no_history());
                    }
                    // The earliest change after to_rev for each key holds
                    // that key's value at to_rev.
                    let mut targets: BTreeMap<String, Option<String>> = BTreeMap::new();
                    let mut q = conn.prepare_cached(
                        "SELECT key, prev, prev_kind FROM changes WHERE space = ?1 AND rev > ?2 ORDER BY rev ASC, key ASC",
                    )?;
                    let mut rows = q.query(params![space.as_str(), to_rev])?;
                    while let Some(r) = rows.next()? {
                        let key: String = r.get(0)?;
                        if targets.contains_key(&key) || !set.contains(&key) {
                            continue;
                        }
                        let kind: Option<i64> = r.get(2)?;
                        let prev: Option<String> = r.get(1)?;
                        let want = match kind {
                            Some(1) => Some(prev.ok_or_else(|| DataError::Unavailable("a kept value is missing".into()))?),
                            Some(0) => None,
                            _ => return Err(no_history()),
                        };
                        targets.insert(key, want);
                    }
                    let (ops, written, deleted, families, listed) = plan_ops(&live, targets, can_write)?;
                    Ok(RestorePlan {
                        ops,
                        to_rev: Some(to_rev),
                        written,
                        deleted,
                        families,
                        keys: listed,
                        set: set.clone(),
                        source,
                    })
                }
            }
        })
    }

    /// Plan and write a restore as one new rev whose changes carry its id
    /// (GH5, GH9). A write that lands between plan and write makes the plan
    /// stale; it is planned again, at most [`RESTORE_TRIES`] times.
    pub fn restore(
        &self,
        space: &Space,
        target: &RestoreTarget,
        set: &KeySet,
        can_write: &dyn Fn(&str) -> bool,
        by: &Actor,
    ) -> Result<Restored, DataError> {
        let mut tries = 0;
        loop {
            tries += 1;
            let plan = self.restore_plan(space, target, set, can_write)?;
            if plan.ops.is_empty() {
                return Ok(Restored { id: None, rev: None, plan });
            }
            let mark = RestoreMark {
                id: format!("rs-{}", super::tx::random_hex(6)),
                source: plan.source.clone(),
                prefixes: plan.set.prefixes.clone(),
                except: plan.set.except.clone(),
                written: plan.written,
                deleted: plan.deleted,
                families: plan.families.clone(),
            };
            match self.write_marked(space, &plan.ops, by, &mark) {
                Ok(rev) => return Ok(Restored { id: Some(mark.id), rev: Some(rev), plan }),
                Err(DataError::Conflict { .. }) if tries < RESTORE_TRIES => continue,
                Err(e) => return Err(e),
            }
        }
    }

    /// `store.snapshot`'s JSON: the set's keys at one rev (one read
    /// transaction). `can_read`: a key the caller can't read in the set is
    /// `not_granted` (a snapshot never silently leaves keys out, R1).
    pub fn snapshot_doc(
        &self,
        space: &Space,
        set: &KeySet,
        label: Option<&str>,
        can_read: &dyn Fn(&str) -> bool,
    ) -> Result<(Vec<u8>, i64, usize), DataError> {
        if !space.is_data() {
            return Err(DataError::BadRequest("only a widget's shared data (data:) has snapshots".into()));
        }
        let at = self.now();
        let (rev, items) = self.with(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
            let rev: i64 = tx
                .query_row("SELECT rev FROM spaces WHERE space = ?1", [space.as_str()], |r| r.get(0))
                .optional()?
                .unwrap_or(0);
            let mut items: Vec<Json> = Vec::new();
            {
                let mut q = tx.prepare_cached("SELECT key, value FROM entries WHERE space = ?1 AND deleted = 0 ORDER BY key")?;
                let mut rows = q.query([space.as_str()])?;
                while let Some(r) = rows.next()? {
                    let key: String = r.get(0)?;
                    if !set.contains(&key) {
                        continue;
                    }
                    if !can_read(&key) {
                        return Err(DataError::NotGranted { key: None });
                    }
                    let value = parse_text(r.get(1)?)?;
                    items.push(json!({ "key": key, "value": value }));
                }
            }
            tx.commit()?;
            Ok((rev, items))
        })?;
        let n = items.len();
        let doc = json!({
            "format": SNAPSHOT_FORMAT,
            "v": SNAPSHOT_VERSION,
            "widget": space.widget(),
            "prefixes": set.prefixes,
            "except": set.except,
            "rev": rev,
            "at": at,
            "label": label,
            "items": items,
        });
        let bytes = serde_json::to_vec(&doc).map_err(|e| DataError::Unavailable(format!("snapshot: {e}")))?;
        if bytes.len() > super::blobs::MAX_BLOB_BYTES {
            return Err(DataError::TooLarge(format!(
                "a snapshot is at most {} MiB; these keys are {} bytes (snapshot fewer prefixes)",
                super::blobs::MAX_BLOB_BYTES / (1024 * 1024),
                bytes.len()
            )));
        }
        Ok((bytes, rev, n))
    }

    /// The hourly trim (GH2): past `max` bytes of before-values in a space,
    /// clear the oldest revs' `prev` (whole revs; the rows stay). Restores
    /// older than the change log go too. Answers the rows trimmed.
    pub fn trim_history(&self, max: i64) -> Result<usize, DataError> {
        let now = self.now();
        self.with(|conn| {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let over: Vec<(String, i64)> = {
                let mut q = tx.prepare(
                    "SELECT space, SUM(LENGTH(CAST(prev AS BLOB))) AS b FROM changes WHERE prev IS NOT NULL
                     GROUP BY space HAVING b > ?1",
                )?;
                let r = q.query_map([max], |r| Ok((r.get(0)?, r.get(1)?)))?;
                r.collect::<Result<_, _>>()?
            };
            let mut trimmed = 0usize;
            for (space, total) in over {
                let cutoff: Option<i64> = {
                    let mut q = tx.prepare(
                        "SELECT rev, SUM(LENGTH(CAST(prev AS BLOB))) FROM changes WHERE space = ?1 AND prev IS NOT NULL
                         GROUP BY rev ORDER BY rev ASC",
                    )?;
                    let mut rows = q.query([&space])?;
                    let mut dropped = 0i64;
                    let mut cut = None;
                    while let Some(r) = rows.next()? {
                        dropped += r.get::<_, i64>(1)?;
                        cut = Some(r.get::<_, i64>(0)?);
                        if total - dropped <= max {
                            break;
                        }
                    }
                    cut
                };
                if let Some(c) = cutoff {
                    trimmed += tx.execute(
                        "UPDATE changes SET prev = NULL, prev_kind = NULL WHERE space = ?1 AND rev <= ?2 AND prev_kind IS NOT NULL",
                        params![space, c],
                    )?;
                }
            }
            tx.execute("DELETE FROM restores WHERE at < ?1", [now - CHANGE_LOG_KEEP_SECS])?;
            tx.commit()?;
            Ok(trimmed)
        })
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
