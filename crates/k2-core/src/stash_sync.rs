//! Prompt stash sync between a person's devices (stash-sync-v1, 0.46.7).
//!
//! PRD: `.k2/prds/prd-stash-sync-v1.md`; the phone contract:
//! `.k2/prds/contract-stash-sync-v1.md`. The record is the spec's shared v1
//! record (`spec-prompt-stash-v1.md`), extended additively: `updated_by`
//! (the device that made `updated_at`, always set here from the pushing
//! device), and per attachment `device_id` + `path` (where the file is; the
//! bytes never come here).
//!
//! **Model.** Mirrors Homes sync (`crate::homes_sync`): every device keeps
//! its own working copy and syncs with this server, which is only the sync
//! peer. One set of records per person, each last-writer-wins on
//! (`updated_at`, `updated_by`) with a tombstone winning a full tie; deletes
//! are tombstones (text, attachments and quotes cleared), kept
//! [`TOMBSTONE_KEEP_MS`]. Devices are the Homes registry (`homes_devices`):
//! one registry per person.
//!
//! **Cursor.** A per-person monotonic `seq`, bumped once by a push that
//! changed something. A pull `since` a cursor returns every record with a
//! larger `seq` (tombstones too); no `since`, one below the tombstone floor,
//! or one from the future returns the whole live set (`full`).
//!
//! **Cap.** At most [`LIVE_CAP`] live records per person: a record that
//! would become live past it is rejected `cap_reached`. A push applies its
//! tombstones first, so "remove the oldest, then save" fits in one push.
//! At most [`MAX_ROWS`] stored records per person, tombstones included: the
//! oldest tombstones go early (raising the floor); a live record never does.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

/// `/boot-status` feature key.
pub const FEATURE: &str = "stash-sync-v1";

/// Tombstones older than this (by `updated_at`) are collected on write.
pub const TOMBSTONE_KEEP_MS: i64 = 90 * 24 * 60 * 60 * 1000;
/// A record stamped further than this in the future is refused.
pub const MAX_FUTURE_SKEW_MS: i64 = 24 * 60 * 60 * 1000;
/// The most records one push may carry.
pub const MAX_CHANGES: usize = 200;
/// The most live records a person may have (the spec's cap).
pub const LIVE_CAP: i64 = 50;
/// The most stored records per person, tombstones included.
pub const MAX_ROWS: i64 = 2000;
/// `text` limit, in characters (Unicode scalar values).
pub const TEXT_MAX: usize = 20_000;
pub const ATTACHMENTS_MAX: usize = 10;
pub const QUOTES_MAX: usize = 20;
/// One record's JSON, as stored.
pub const RECORD_MAX_BYTES: usize = 128 * 1024;
pub const ATTACHMENT_ID_MAX: usize = 128;
pub const NAME_MAX: usize = 255;
pub const TYPE_MAX: usize = 255;
pub const PATH_MAX: usize = 4096;
pub const SOURCE_FIELD_MAX: usize = 300;

const SOURCE_KEYS: [&str; 5] = ["server_id", "server_label", "user", "agent", "agent_name"];

/// True when migration 0257's tables (and 0193's device registry) exist.
pub fn tables_exist(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
         AND name IN ('stash_items', 'stash_people', 'homes_devices')",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n == 3)
    .unwrap_or(false)
}

/// Why a pushed record was refused (`rejected[].reason`).
pub mod reason {
    pub const NOT_AN_OBJECT: &str = "not_an_object";
    pub const UNSUPPORTED_VERSION: &str = "unsupported_version";
    pub const INVALID_ID: &str = "invalid_id";
    pub const INVALID_TEXT: &str = "invalid_text";
    pub const TEXT_TOO_LONG: &str = "text_too_long";
    pub const INVALID_ATTACHMENT: &str = "invalid_attachment";
    pub const TOO_MANY_ATTACHMENTS: &str = "too_many_attachments";
    pub const INVALID_QUOTE: &str = "invalid_quote";
    pub const TOO_MANY_QUOTES: &str = "too_many_quotes";
    pub const INVALID_SOURCE: &str = "invalid_source";
    pub const INVALID_CREATED_AT: &str = "invalid_created_at";
    pub const INVALID_UPDATED_AT: &str = "invalid_updated_at";
    pub const FUTURE_UPDATED_AT: &str = "future_updated_at";
    pub const INVALID_DELETED_AT: &str = "invalid_deleted_at";
    pub const TOO_LARGE: &str = "too_large";
    pub const CAP_REACHED: &str = "cap_reached";
}

/// One stash record as stored (and as accepted from a push).
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub updated_by: String,
    pub deleted_at: Option<i64>,
    /// The whole record object (wire form), `updated_by` included.
    pub body: Map<String, Value>,
}

impl Record {
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }

    /// Is `self` the winner over `other`? `updated_at`, then `updated_by`
    /// (string compare), then a tombstone wins a full tie. A full tie with
    /// the same live/deleted state is "not newer" (an idempotent retry).
    pub fn beats(&self, other: &Record) -> bool {
        use std::cmp::Ordering::*;
        match self.updated_at.cmp(&other.updated_at) {
            Greater => true,
            Less => false,
            Equal => match self.updated_by.as_str().cmp(other.updated_by.as_str()) {
                Greater => true,
                Less => false,
                Equal => !self.is_live() && other.is_live(),
            },
        }
    }

    pub fn to_json(&self) -> Value {
        Value::Object(self.body.clone())
    }
}

/// A lowercase uuid: 8-4-4-4-12 hex digits.
pub fn valid_record_id(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

fn positive_int(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_i64).filter(|t| *t > 0)
}

/// Optional string field: absent stays absent, null stays null, "" becomes
/// null; a string longer than `max` characters (or with a NUL) fails.
fn opt_string(o: &mut Map<String, Value>, key: &str, max: usize) -> Result<(), ()> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::String(s)) if s.is_empty() => {
            o.insert(key.to_string(), Value::Null);
            Ok(())
        }
        Some(Value::String(s)) if s.chars().count() <= max && !s.contains('\0') => Ok(()),
        Some(_) => Err(()),
    }
}

fn clean_attachment(v: &Value) -> Result<Value, &'static str> {
    let bad = reason::INVALID_ATTACHMENT;
    let mut o = v.as_object().cloned().ok_or(bad)?;
    match o.get("id").and_then(Value::as_str) {
        Some(id) if !id.is_empty() && id.len() <= ATTACHMENT_ID_MAX && !id.chars().any(char::is_control) => {}
        _ => return Err(bad),
    }
    match o.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() && n.chars().count() <= NAME_MAX && !n.contains('\0') => {}
        _ => return Err(bad),
    }
    match o.get("type") {
        None | Some(Value::Null) => {
            o.insert("type".into(), json!(""));
        }
        Some(Value::String(t)) if t.chars().count() <= TYPE_MAX => {}
        Some(_) => return Err(bad),
    }
    if !o.get("size").is_some_and(|s| s.as_u64().is_some()) {
        return Err(bad);
    }
    match o.get("device_id") {
        None | Some(Value::Null) => {}
        Some(Value::String(d)) if d.is_empty() => {
            o.insert("device_id".into(), Value::Null);
        }
        Some(Value::String(d)) if crate::homes_sync::valid_device_id(d) => {}
        Some(_) => return Err(bad),
    }
    opt_string(&mut o, "path", PATH_MAX).map_err(|_| bad)?;
    Ok(Value::Object(o))
}

fn clean_quote(v: &Value) -> Result<Value, &'static str> {
    let bad = reason::INVALID_QUOTE;
    let mut o = v.as_object().cloned().ok_or(bad)?;
    if !o.get("text").is_some_and(Value::is_string) {
        return Err(bad);
    }
    if !o.contains_key("from") {
        o.insert("from".into(), Value::Null);
    }
    opt_string(&mut o, "from", SOURCE_FIELD_MAX).map_err(|_| bad)?;
    Ok(Value::Object(o))
}

fn clean_source(v: Option<&Value>) -> Result<Value, &'static str> {
    let bad = reason::INVALID_SOURCE;
    let mut o = match v {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(o)) => o.clone(),
        Some(_) => return Err(bad),
    };
    for k in SOURCE_KEYS {
        if !o.contains_key(k) {
            o.insert(k.into(), Value::Null);
        }
        opt_string(&mut o, k, SOURCE_FIELD_MAX).map_err(|_| bad)?;
    }
    // A device-local id names a different box on every device: unknown.
    if o.get("server_id").and_then(Value::as_str).is_some_and(|s| s.eq_ignore_ascii_case("local")) {
        o.insert("server_id".into(), Value::Null);
    }
    Ok(Value::Object(o))
}

/// Parse one pushed record into a [`Record`] from `device_id`, or the
/// reason it is refused. Unknown fields are kept as sent.
pub fn parse_change(v: &Value, device_id: &str, now_ms: i64) -> Result<Record, &'static str> {
    let mut o = v.as_object().cloned().ok_or(reason::NOT_AN_OBJECT)?;
    if o.get("v").and_then(Value::as_i64) != Some(1) {
        return Err(reason::UNSUPPORTED_VERSION);
    }
    let id = o
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| valid_record_id(s))
        .ok_or(reason::INVALID_ID)?
        .to_string();
    let created_at = positive_int(o.get("created_at")).ok_or(reason::INVALID_CREATED_AT)?;
    let updated_at = positive_int(o.get("updated_at")).ok_or(reason::INVALID_UPDATED_AT)?;
    if updated_at > now_ms + MAX_FUTURE_SKEW_MS {
        return Err(reason::FUTURE_UPDATED_AT);
    }
    let deleted_at = match o.get("deleted_at") {
        None | Some(Value::Null) => None,
        v @ Some(_) => Some(positive_int(v).ok_or(reason::INVALID_DELETED_AT)?),
    };
    let source = clean_source(o.get("source"))?;
    o.insert("source".into(), source);
    o.insert("deleted_at".into(), deleted_at.map_or(Value::Null, Value::from));
    o.insert("updated_by".into(), json!(device_id));
    if deleted_at.is_some() {
        // A tombstone carries no content (spec: text and attachments cleared).
        if !matches!(o.get("text"), None | Some(Value::Null) | Some(Value::String(_))) {
            return Err(reason::INVALID_TEXT);
        }
        o.insert("text".into(), json!(""));
        o.insert("attachments".into(), json!([]));
        o.insert("quotes".into(), json!([]));
    } else {
        match o.get("text") {
            Some(Value::String(t)) if t.chars().count() > TEXT_MAX => return Err(reason::TEXT_TOO_LONG),
            Some(Value::String(_)) => {}
            _ => return Err(reason::INVALID_TEXT),
        }
        let attachments = match o.get("attachments") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) if a.len() > ATTACHMENTS_MAX => return Err(reason::TOO_MANY_ATTACHMENTS),
            Some(Value::Array(a)) => a.iter().map(clean_attachment).collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(reason::INVALID_ATTACHMENT),
        };
        let quotes = match o.get("quotes") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(q)) if q.len() > QUOTES_MAX => return Err(reason::TOO_MANY_QUOTES),
            Some(Value::Array(q)) => q.iter().map(clean_quote).collect::<Result<Vec<_>, _>>()?,
            Some(_) => return Err(reason::INVALID_QUOTE),
        };
        o.insert("attachments".into(), Value::Array(attachments));
        o.insert("quotes".into(), Value::Array(quotes));
    }
    if Value::Object(o.clone()).to_string().len() > RECORD_MAX_BYTES {
        return Err(reason::TOO_LARGE);
    }
    Ok(Record { id, created_at, updated_at, updated_by: device_id.to_string(), deleted_at, body: o })
}

// ── Storage ────────────────────────────────────────────────────────────

const COLS: &str = "id, record, created_at, updated_at, updated_by, deleted_at";

fn record_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Record> {
    let raw: String = r.get(1)?;
    let body = match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(o)) => o,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                "stash record is not a JSON object".into(),
            ))
        }
    };
    Ok(Record {
        id: r.get(0)?,
        body,
        created_at: r.get(2)?,
        updated_at: r.get(3)?,
        updated_by: r.get(4)?,
        deleted_at: r.get(5)?,
    })
}

fn get_record(conn: &Connection, person: &str, id: &str) -> rusqlite::Result<Option<Record>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM stash_items WHERE person = ?1 AND id = ?2"),
        params![person, id],
        record_from_row,
    )
    .optional()
}

fn person_seq(conn: &Connection, person: &str) -> rusqlite::Result<(i64, i64)> {
    Ok(conn
        .query_row("SELECT seq, floor FROM stash_people WHERE person = ?1", params![person], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()?
        .unwrap_or((0, 0)))
}

/// A person's live record count.
pub fn live_count(conn: &Connection, person: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COUNT(*) FROM stash_items WHERE person = ?1 AND deleted_at IS NULL",
        params![person],
        |r| r.get(0),
    )
}

// ── Pull ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Pull {
    /// The person's cursor now (pass it back as `since`).
    pub cursor: i64,
    /// True when `changes` is the whole live set: the client replaces.
    pub full: bool,
    pub changes: Vec<Record>,
}

/// A person's changes since `since` (None = the whole live set). Touches
/// the device's `lastSyncAt` in the Homes registry when it is active.
pub fn pull(
    conn: &Connection,
    person: &str,
    since: Option<i64>,
    device_id: Option<&str>,
    now_ms: i64,
) -> rusqlite::Result<Pull> {
    let (seq, floor) = person_seq(conn, person)?;
    let full = match since {
        None => true,
        Some(s) => s < floor || s > seq || s < 0,
    };
    let changes: Vec<Record> = if full {
        let mut st = conn.prepare(&format!(
            "SELECT {COLS} FROM stash_items WHERE person = ?1 AND deleted_at IS NULL ORDER BY seq, id"
        ))?;
        let rows = st.query_map(params![person], record_from_row)?;
        rows.collect::<rusqlite::Result<_>>()?
    } else {
        let mut st =
            conn.prepare(&format!("SELECT {COLS} FROM stash_items WHERE person = ?1 AND seq > ?2 ORDER BY seq, id"))?;
        let rows = st.query_map(params![person, since.unwrap_or(0)], record_from_row)?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if let Some(d) = device_id {
        crate::homes_sync::touch_device(conn, person, d, now_ms)?;
    }
    Ok(Pull { cursor: seq, full, changes })
}

// ── Push ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Rejected {
    pub item: Value,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PushResult {
    pub cursor: i64,
    /// Records written (newer than the server's copy).
    pub applied: usize,
    /// Records the server already held a winning version of.
    pub superseded: usize,
    pub rejected: Vec<Rejected>,
}

impl PushResult {
    pub fn changed(&self) -> bool {
        self.applied > 0
    }
}

/// Merge `changes` from `device_id` into `person`'s stash (one transaction).
/// Tombstones apply before live records, so a removal frees room for a
/// save in the same push.
pub fn push(
    conn: &Connection,
    person: &str,
    device_id: &str,
    changes: &[Value],
    now_ms: i64,
) -> rusqlite::Result<PushResult> {
    let mut rejected = Vec::new();
    let mut parsed: Vec<(Record, &Value)> = Vec::new();
    for c in changes {
        match parse_change(c, device_id, now_ms) {
            Ok(r) => parsed.push((r, c)),
            Err(reason) => rejected.push(Rejected { item: c.clone(), reason }),
        }
    }
    // Tombstones first; stable within each group.
    parsed.sort_by_key(|(r, _)| r.is_live());

    let tx = conn.unchecked_transaction()?;
    let (seq, mut floor) = person_seq(&tx, person)?;
    let next = seq + 1;
    let mut live = live_count(&tx, person)?;
    let mut applied = 0usize;
    let mut superseded = 0usize;
    for (rec, sent) in parsed {
        let existing = get_record(&tx, person, &rec.id)?;
        if let Some(ex) = &existing {
            if !rec.beats(ex) {
                superseded += 1;
                continue;
            }
        }
        let was_live = existing.as_ref().is_some_and(Record::is_live);
        if rec.is_live() && !was_live && live >= LIVE_CAP {
            rejected.push(Rejected { item: sent.clone(), reason: reason::CAP_REACHED });
            continue;
        }
        tx.execute(
            "INSERT INTO stash_items (person, id, record, created_at, updated_at, updated_by, deleted_at, seq) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (person, id) DO UPDATE SET record = excluded.record, created_at = excluded.created_at, \
               updated_at = excluded.updated_at, updated_by = excluded.updated_by, \
               deleted_at = excluded.deleted_at, seq = excluded.seq",
            params![
                person,
                rec.id,
                rec.to_json().to_string(),
                rec.created_at,
                rec.updated_at,
                rec.updated_by,
                rec.deleted_at,
                next
            ],
        )?;
        match (was_live, rec.is_live()) {
            (false, true) => live += 1,
            (true, false) => live -= 1,
            _ => {}
        }
        applied += 1;
    }
    let cursor = if applied > 0 {
        floor = floor.max(collect_tombstones(&tx, person, now_ms)?);
        tx.execute(
            "INSERT INTO stash_people (person, seq, floor) VALUES (?1, ?2, ?3) \
             ON CONFLICT (person) DO UPDATE SET seq = excluded.seq, floor = excluded.floor",
            params![person, next, floor],
        )?;
        next
    } else {
        seq
    };
    crate::homes_sync::touch_device(&tx, person, device_id, now_ms)?;
    tx.commit()?;
    Ok(PushResult { cursor, applied, superseded, rejected })
}

/// Drop tombstones older than [`TOMBSTONE_KEEP_MS`], then the oldest
/// tombstones past [`MAX_ROWS`] stored records. Returns the largest `seq`
/// dropped (0 when none): the new floor candidate.
fn collect_tombstones(tx: &Connection, person: &str, now_ms: i64) -> rusqlite::Result<i64> {
    let cutoff = now_ms - TOMBSTONE_KEEP_MS;
    let mut max_seq: i64 = tx
        .query_row(
            "SELECT MAX(seq) FROM stash_items WHERE person = ?1 AND deleted_at IS NOT NULL AND updated_at < ?2",
            params![person, cutoff],
            |r| r.get::<_, Option<i64>>(0),
        )?
        .unwrap_or(0);
    tx.execute(
        "DELETE FROM stash_items WHERE person = ?1 AND deleted_at IS NOT NULL AND updated_at < ?2",
        params![person, cutoff],
    )?;
    let total: i64 = tx.query_row("SELECT COUNT(*) FROM stash_items WHERE person = ?1", params![person], |r| r.get(0))?;
    if total > MAX_ROWS {
        let over = total - MAX_ROWS;
        let ids: Vec<(String, i64)> = {
            let mut st = tx.prepare(
                "SELECT id, seq FROM stash_items WHERE person = ?1 AND deleted_at IS NOT NULL \
                 ORDER BY updated_at, seq, id LIMIT ?2",
            )?;
            let rows = st.query_map(params![person, over], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, s) in ids {
            tx.execute("DELETE FROM stash_items WHERE person = ?1 AND id = ?2", params![person, id])?;
            max_seq = max_seq.max(s);
        }
    }
    Ok(max_seq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::homes_sync::{device_state, register_device, remove_device, DeviceState};

    const NOW: i64 = 1_760_000_000_000;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("mem db");
        for sql in [include_str!("../drizzle_sql/0193_homes_sync.sql"), include_str!("../drizzle_sql/0257_stash_sync.sql")] {
            conn.execute_batch(sql.replace("--> statement-breakpoint", "").as_str()).expect("migration applies");
        }
        assert!(tables_exist(&conn));
        conn
    }

    /// A uuid for test record `n`.
    fn uid(n: u32) -> String {
        format!("00000000-0000-4000-8000-{n:012x}")
    }

    fn rec(n: u32, text: &str, at: i64) -> Value {
        json!({
            "v": 1, "id": uid(n), "text": text, "attachments": [], "quotes": [],
            "source": {"server_id": "rosson.k2.dev", "server_label": null, "user": "rosson", "agent": null, "agent_name": null},
            "created_at": at, "updated_at": at, "deleted_at": null
        })
    }

    fn edit(mut v: Value, text: &str, at: i64) -> Value {
        v["text"] = json!(text);
        v["updated_at"] = json!(at);
        v
    }

    fn tomb(mut v: Value, at: i64) -> Value {
        v["deleted_at"] = json!(at);
        v["updated_at"] = json!(at);
        v
    }

    fn texts(c: &Connection, person: &str) -> Vec<String> {
        let mut v: Vec<String> = pull(c, person, None, None, NOW)
            .expect("pull")
            .changes
            .iter()
            .map(|r| r.body["text"].as_str().expect("text").to_string())
            .collect();
        v.sort();
        v
    }

    fn reasons(r: &PushResult) -> Vec<&'static str> {
        r.rejected.iter().map(|x| x.reason).collect()
    }

    #[test]
    fn newest_updated_at_wins_then_larger_device_then_tombstone() {
        let c = db();
        push(&c, "owner", "laptop", &[rec(1, "first", NOW)], NOW).expect("seed");
        let r = push(&c, "owner", "phone", &[edit(rec(1, "", NOW), "phone", NOW + 20)], NOW).expect("phone");
        assert_eq!((r.applied, r.superseded), (1, 0));
        let r = push(&c, "owner", "laptop", &[edit(rec(1, "", NOW), "laptop", NOW + 10)], NOW).expect("late laptop");
        assert_eq!((r.applied, r.superseded), (0, 1), "an older edit loses whatever order it arrives in");
        assert_eq!(texts(&c, "owner"), vec!["phone"]);
        // Same millisecond: the larger device id wins.
        push(&c, "owner", "a-dev", &[edit(rec(1, "", NOW), "from a", NOW + 30)], NOW).expect("a");
        let r = push(&c, "owner", "b-dev", &[edit(rec(1, "", NOW), "from b", NOW + 30)], NOW).expect("b");
        assert_eq!(r.applied, 1, "b > a on a tie");
        let r = push(&c, "owner", "a-dev", &[edit(rec(1, "", NOW), "from a again", NOW + 30)], NOW).expect("a again");
        assert_eq!(r.applied, 0, "a < b on a tie");
        assert_eq!(texts(&c, "owner"), vec!["from b"]);
        // A full tie (same stamp, same device): the tombstone wins.
        let r = push(&c, "owner", "b-dev", &[tomb(rec(1, "", NOW), NOW + 30)], NOW).expect("tie tomb");
        assert_eq!(r.applied, 1, "a tombstone wins a full tie");
        let r = push(&c, "owner", "b-dev", &[edit(rec(1, "", NOW), "back?", NOW + 30)], NOW).expect("tie live");
        assert_eq!((r.applied, r.superseded), (0, 1), "a live copy never beats its tombstone on a full tie");
        assert!(texts(&c, "owner").is_empty());
        // A retry of the same record is a no-op: no write, no cursor bump.
        let cur = pull(&c, "owner", None, None, NOW).unwrap().cursor;
        let r = push(&c, "owner", "b-dev", &[tomb(rec(1, "", NOW), NOW + 30)], NOW).expect("retry");
        assert_eq!((r.applied, r.superseded, r.cursor), (0, 1, cur));
        // updated_by is always the pushing device, never the body's.
        let mut forged = rec(2, "x", NOW);
        forged["updated_by"] = json!("zzzz");
        push(&c, "owner", "phone", &[forged], NOW).expect("forged");
        let p = pull(&c, "owner", None, None, NOW).unwrap();
        assert_eq!(p.changes[0].body["updated_by"], "phone");
    }

    #[test]
    fn tombstones_beat_older_edits_clear_content_and_reach_stale_devices() {
        let c = db();
        let mut r1 = rec(1, "secret draft", NOW);
        r1["attachments"] = json!([{"id": "a1", "name": "shot.png", "type": "image/png", "size": 10, "device_id": "laptop", "path": "/Users/r/shot.png"}]);
        push(&c, "owner", "laptop", &[r1.clone()], NOW).expect("seed");
        let since = pull(&c, "owner", None, None, NOW).unwrap().cursor;
        push(&c, "owner", "phone", &[tomb(r1.clone(), NOW + 10)], NOW).expect("delete");
        let r = push(&c, "owner", "laptop", &[edit(r1.clone(), "older edit", NOW + 5)], NOW).expect("stale edit");
        assert_eq!(r.applied, 0, "a tombstone beats an older edit");
        // A device behind the delete learns of it (tombstones ride incremental pulls).
        let p = pull(&c, "owner", Some(since), None, NOW).unwrap();
        assert!(!p.full);
        assert_eq!(p.changes.len(), 1);
        let t = &p.changes[0].body;
        assert_eq!(t["deleted_at"], NOW + 10);
        assert_eq!((t["text"].as_str(), t["attachments"].as_array().map(Vec::len)), (Some(""), Some(0)), "content cleared");
        // A full pull leaves tombstones out.
        assert!(pull(&c, "owner", None, None, NOW).unwrap().changes.is_empty());
        // An edit made after the delete brings it back.
        let r = push(&c, "owner", "laptop", &[edit(r1, "again", NOW + 20)], NOW).expect("re-add");
        assert_eq!(r.applied, 1);
        assert_eq!(texts(&c, "owner"), vec!["again"]);
    }

    #[test]
    fn tombstones_are_kept_90_days_then_collected_and_the_floor_forces_a_full_pull() {
        let c = db();
        push(&c, "owner", "d", &[rec(1, "a", NOW), rec(2, "b", NOW)], NOW).expect("seed");
        let old = pull(&c, "owner", None, None, NOW).unwrap().cursor;
        push(&c, "owner", "d", &[tomb(rec(1, "", NOW), NOW + 1)], NOW).expect("tomb");
        // 89 days later a write keeps it.
        let soon = NOW + TOMBSTONE_KEEP_MS - 24 * 3600 * 1000;
        push(&c, "owner", "d", &[edit(rec(2, "", NOW), "b2", soon)], soon).expect("89d");
        let p = pull(&c, "owner", Some(old), None, soon).unwrap();
        assert!(!p.full && p.changes.iter().any(|r| !r.is_live()), "still there at 89 days");
        // 91 days later a write collects it, and an old cursor gets the whole set.
        let later = NOW + TOMBSTONE_KEEP_MS + 10;
        push(&c, "owner", "d", &[edit(rec(2, "", NOW), "b3", later)], later).expect("91d");
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM stash_items WHERE person = 'owner' AND deleted_at IS NOT NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "collected");
        let p = pull(&c, "owner", Some(old), None, later).unwrap();
        assert!(p.full, "a cursor below the floor gets the whole live set");
        assert_eq!(texts(&c, "owner"), vec!["b3"]);
        // A cursor from the future (another server, a reset DB) is full too.
        assert!(pull(&c, "owner", Some(p.cursor + 5), None, later).unwrap().full);
    }

    #[test]
    fn the_50_live_cap_is_enforced_on_push_and_tombstones_free_room_first() {
        let c = db();
        let fifty: Vec<Value> = (1..=50).map(|n| rec(n, &format!("t{n}"), NOW + n as i64)).collect();
        let r = push(&c, "owner", "d", &fifty, NOW).expect("fill");
        assert_eq!(r.applied, 50);
        let r = push(&c, "owner", "d", &[rec(51, "51st", NOW + 100)], NOW).expect("51st");
        assert_eq!((r.applied, reasons(&r)), (0, vec![reason::CAP_REACHED]), "the 51st live record is refused");
        assert_eq!(r.rejected[0].item["id"], uid(51), "the item is echoed as sent");
        // Editing a live one at the cap is fine.
        let r = push(&c, "owner", "d", &[edit(rec(3, "", NOW + 3), "t3 edited", NOW + 101)], NOW).expect("edit");
        assert_eq!(r.applied, 1);
        // Remove the oldest + save, in one push (live listed first): fits.
        let r = push(&c, "owner", "d", &[rec(51, "51st", NOW + 102), tomb(rec(1, "", NOW + 1), NOW + 102)], NOW)
            .expect("swap");
        assert_eq!((r.applied, r.rejected.len()), (2, 0), "{:?}", reasons(&r));
        assert_eq!(live_count(&c, "owner").unwrap(), 50);
        // Bringing a tombstone back to life counts as a new live record.
        let r = push(&c, "owner", "d", &[edit(rec(1, "", NOW + 1), "back", NOW + 200)], NOW).expect("revive");
        assert_eq!(reasons(&r), vec![reason::CAP_REACHED]);
        // The cap is per person.
        let r = push(&c, "gina", "g", &[rec(60, "gina's", NOW)], NOW).expect("gina");
        assert_eq!(r.applied, 1);
    }

    #[test]
    fn people_are_isolated() {
        let c = db();
        push(&c, "owner", "d1", &[rec(1, "owner's", NOW)], NOW).expect("owner");
        push(&c, "gina", "d2", &[rec(2, "gina's", NOW)], NOW).expect("gina");
        assert_eq!(texts(&c, "owner"), vec!["owner's"]);
        assert_eq!(texts(&c, "gina"), vec!["gina's"]);
        // The same record id under another person is a different record.
        push(&c, "gina", "d2", &[tomb(rec(1, "", NOW), NOW + 5)], NOW).expect("gina tomb of owner's id");
        assert_eq!(texts(&c, "owner"), vec!["owner's"], "gina can't delete the owner's record");
        let p = pull(&c, "gina", Some(0), None, NOW).unwrap();
        assert!(p.changes.iter().all(|r| r.body["text"] != "owner's"));
    }

    #[test]
    fn validation_reasons_and_field_rules() {
        let c = db();
        let mut cases: Vec<(Value, &str)> = vec![
            (json!("x"), reason::NOT_AN_OBJECT),
            (json!({"v": 2, "id": uid(1)}), reason::UNSUPPORTED_VERSION),
            ({ let mut v = rec(1, "x", NOW); v["id"] = json!("6F1C2A8E-4B7D-4E1A-9C3F-2D5E8A7B1C90"); v }, reason::INVALID_ID),
            ({ let mut v = rec(1, "x", NOW); v["id"] = json!("not-a-uuid"); v }, reason::INVALID_ID),
            ({ let mut v = rec(1, "x", NOW); v["text"] = json!(5); v }, reason::INVALID_TEXT),
            (rec(1, &"é".repeat(TEXT_MAX + 1), NOW), reason::TEXT_TOO_LONG),
            ({ let mut v = rec(1, "x", NOW); v["attachments"] = json!([{"id": "a", "name": "", "type": "", "size": 1}]); v }, reason::INVALID_ATTACHMENT),
            ({ let mut v = rec(1, "x", NOW); v["attachments"] = json!([{"id": "a", "name": "n", "type": "", "size": -1}]); v }, reason::INVALID_ATTACHMENT),
            ({ let mut v = rec(1, "x", NOW); v["attachments"] = json!([{"id": "a", "name": "n", "type": "", "size": 1, "device_id": "bad id"}]); v }, reason::INVALID_ATTACHMENT),
            ({ let mut v = rec(1, "x", NOW); v["attachments"] = json!([{"id": "a", "name": "n", "type": "", "size": 1, "path": "x".repeat(PATH_MAX + 1)}]); v }, reason::INVALID_ATTACHMENT),
            ({ let mut v = rec(1, "x", NOW); v["attachments"] = Value::Array((0..11).map(|i| json!({"id": format!("a{i}"), "name": "n", "type": "", "size": 1})).collect()); v }, reason::TOO_MANY_ATTACHMENTS),
            ({ let mut v = rec(1, "x", NOW); v["quotes"] = json!([{"from": "x"}]); v }, reason::INVALID_QUOTE),
            ({ let mut v = rec(1, "x", NOW); v["quotes"] = Value::Array((0..21).map(|_| json!({"text": "q", "from": null})).collect()); v }, reason::TOO_MANY_QUOTES),
            ({ let mut v = rec(1, "x", NOW); v["source"] = json!("rosson.k2.dev"); v }, reason::INVALID_SOURCE),
            ({ let mut v = rec(1, "x", NOW); v["created_at"] = json!(0); v }, reason::INVALID_CREATED_AT),
            ({ let mut v = rec(1, "x", NOW); v["updated_at"] = json!("soon"); v }, reason::INVALID_UPDATED_AT),
            (rec(1, "x", NOW + MAX_FUTURE_SKEW_MS + 1), reason::FUTURE_UPDATED_AT),
            ({ let mut v = rec(1, "x", NOW); v["deleted_at"] = json!(true); v }, reason::INVALID_DELETED_AT),
        ];
        let mut big = rec(1, "x", NOW);
        big["quotes"] = Value::Array((0..20).map(|_| json!({"text": "q".repeat(7000), "from": null})).collect());
        cases.push((big, reason::TOO_LARGE));
        for (v, want) in cases {
            assert_eq!(parse_change(&v, "d", NOW).map(|_| ()), Err(want), "{v}");
        }
        assert!(push(&c, "owner", "d", &[], NOW).unwrap().rejected.is_empty());

        // Field rules: "" → null, server_id "local" → null, old records
        // without device_id/path stay valid, unknown fields are kept.
        let mut v = rec(7, "hi", NOW);
        v["source"] = json!({"server_id": "local", "server_label": "", "user": "rosson"});
        v["attachments"] = json!([
            {"id": "a1", "name": "old.png", "type": "image/png", "size": 3},
            {"id": "a2", "name": "new.png", "type": "", "size": 4, "device_id": "d-1", "path": "/tmp/new.png", "later": 1}
        ]);
        v["pinned"] = json!(true);
        let r = parse_change(&v, "d", NOW).expect("valid");
        let s = &r.body["source"];
        assert_eq!((s["server_id"].clone(), s["server_label"].clone(), s["agent"].clone()), (Value::Null, Value::Null, Value::Null));
        assert_eq!(s["user"], "rosson");
        assert!(r.body["attachments"][0].get("device_id").is_none(), "absent stays absent");
        assert_eq!(r.body["attachments"][1]["path"], "/tmp/new.png");
        assert_eq!(r.body["attachments"][1]["later"], 1);
        assert_eq!(r.body["pinned"], true, "unknown fields kept");
        // A tombstone needs no valid content (it is cleared).
        let mut t = tomb(rec(8, "", NOW), NOW + 1);
        t["attachments"] = json!("junk");
        assert!(parse_change(&t, "d", NOW).is_ok());
    }

    #[test]
    fn stored_rows_are_capped_by_collecting_the_oldest_tombstones_early() {
        let c = db();
        // MAX_ROWS tombstones, written in batches of MAX_CHANGES.
        let total = MAX_ROWS as u32;
        let mut n = 1u32;
        while n <= total {
            let batch: Vec<Value> =
                (n..(n + MAX_CHANGES as u32).min(total + 1)).map(|i| tomb(rec(i, "", NOW + i as i64), NOW + i as i64)).collect();
            push(&c, "owner", "d", &batch, NOW + total as i64).expect("batch");
            n += MAX_CHANGES as u32;
        }
        let first = pull(&c, "owner", None, None, NOW).unwrap().cursor;
        push(&c, "owner", "d", &[rec(total + 1, "live", NOW + 5000)], NOW + 5000).expect("live");
        let rows: i64 = c.query_row("SELECT COUNT(*) FROM stash_items WHERE person = 'owner'", [], |r| r.get(0)).unwrap();
        assert_eq!(rows, MAX_ROWS, "held at the cap");
        let gone: i64 = c.query_row("SELECT COUNT(*) FROM stash_items WHERE id = ?1", params![uid(1)], |r| r.get(0)).unwrap();
        assert_eq!(gone, 0, "the oldest tombstone went first");
        assert_eq!(texts(&c, "owner"), vec!["live"], "never a live record");
        assert!(pull(&c, "owner", Some(0), None, NOW).unwrap().full, "a cursor before the collected tombstone gets a full pull");
        assert!(!pull(&c, "owner", Some(first), None, NOW).unwrap().full);
    }

    #[test]
    fn devices_are_the_homes_registry() {
        let c = db();
        assert_eq!(device_state(&c, "owner", "phone").unwrap(), DeviceState::Unknown);
        register_device(&c, "owner", "phone", "Rosson's phone", "ios", NOW).unwrap();
        push(&c, "owner", "phone", &[rec(1, "x", NOW)], NOW + 7).expect("push");
        let last: Option<i64> = c
            .query_row("SELECT last_sync_at FROM homes_devices WHERE device_id = 'phone'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(last, Some(NOW + 7), "a stash push touches the shared registry");
        remove_device(&c, "owner", "phone", NOW + 8).unwrap();
        assert_eq!(device_state(&c, "owner", "phone").unwrap(), DeviceState::Removed);
    }
}
