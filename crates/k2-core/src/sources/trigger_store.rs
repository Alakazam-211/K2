//! rusqlite repo for `source_triggers` and `source_trigger_fires` (plus the
//! `source_audit` writer the trigger and approval routes use). Owner: B2.
//!
//! Times are wall seconds. Fire counts for the caps (contract §9 steps
//! 5–8) count only `pending`, `delivered` and `inbox` outcomes; tests and
//! suppressions never count.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::triggers::{TriggerOn, TriggerSpec, TriggerStatus};

/// One `source_trigger_fires` row as `triggers/show` and `triggers/log`
/// return it (never note text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FireRow {
    pub id: String,
    pub at: i64,
    pub incident_id: String,
    pub item_key: Option<String>,
    /// `pending` | `delivered` | `inbox` | `undeliverable` | `suppressed` |
    /// `coalesced` | `test`.
    pub outcome: String,
    pub suppress_reason: Option<String>,
    pub delivered_at: Option<i64>,
    pub woke: Option<bool>,
    pub note_bytes: Option<i64>,
    /// A fixed code (MsgReason / inbox error kind).
    pub detail: Option<String>,
}

/// How many triggers a source has (any status).
pub fn count_for_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.query_row("SELECT COUNT(*) FROM source_triggers WHERE source_id = ?1", params![source_id], |r| {
        r.get::<_, i64>(0)
    })
    .map(|n| n as usize)
}

/// Delete a source's triggers (removal; the fire log ages out on its own).
/// Returns how many went.
pub fn delete_for_source(conn: &Connection, source_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM source_triggers WHERE source_id = ?1", params![source_id])
}

// ── triggers ─────────────────────────────────────────────────────────

/// One `source_triggers` row, typed.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerRow {
    pub id: String,
    pub source_id: String,
    pub on: TriggerOn,
    pub target_project_id: String,
    pub note: Option<String>,
    pub cooldown_secs: u32,
    pub daily_cap: u16,
    pub status: TriggerStatus,
    pub made_by: String,
    pub made_by_project: Option<String>,
    pub approved_by: Option<String>,
    pub approved_at: Option<i64>,
    pub last_fire_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl TriggerRow {
    pub fn spec(&self) -> TriggerSpec {
        TriggerSpec {
            id: self.id.clone(),
            source_id: self.source_id.clone(),
            on: self.on.clone(),
            target_project_id: self.target_project_id.clone(),
            note: self.note.clone(),
            cooldown_secs: self.cooldown_secs,
            daily_cap: self.daily_cap,
            status: self.status,
            made_by: self.made_by.clone(),
        }
    }
}

const T_COLUMNS: &str = "id, source_id, on_kind, stale_for_secs, target_project_id, note, cooldown_secs, \
     daily_cap, status, made_by, made_by_project, approved_by, approved_at, last_fire_at, created_at, updated_at";

fn conv(idx: usize, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, msg.into())
}

fn t_row(r: &Row) -> rusqlite::Result<TriggerRow> {
    let kind: String = r.get(2)?;
    let stale: Option<i64> = r.get(3)?;
    let on = TriggerOn::from_parts(&kind, stale).ok_or_else(|| conv(2, format!("on_kind {kind:?}")))?;
    let status: String = r.get(8)?;
    Ok(TriggerRow {
        id: r.get(0)?,
        source_id: r.get(1)?,
        on,
        target_project_id: r.get(4)?,
        note: r.get(5)?,
        cooldown_secs: r.get(6)?,
        daily_cap: r.get(7)?,
        status: TriggerStatus::from_code(&status).ok_or_else(|| conv(8, format!("status {status:?}")))?,
        made_by: r.get(9)?,
        made_by_project: r.get(10)?,
        approved_by: r.get(11)?,
        approved_at: r.get(12)?,
        last_fire_at: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
    })
}

/// A trigger to create.
pub struct NewTrigger<'a> {
    pub source_id: &'a str,
    pub on: &'a TriggerOn,
    pub target_project_id: &'a str,
    pub note: Option<&'a str>,
    pub cooldown_secs: u32,
    pub daily_cap: u16,
    pub status: TriggerStatus,
    pub made_by: &'a str,
    pub made_by_project: Option<&'a str>,
    pub approved_by: Option<&'a str>,
    pub now: i64,
}

#[derive(Debug, PartialEq)]
pub enum InsertErr {
    /// The unique index (one per source, kind, agent and stale duration).
    Exists,
    Db(String),
}

fn is_unique_violation(e: &rusqlite::Error) -> bool {
    matches!(e, rusqlite::Error::SqliteFailure(f, _) if f.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE)
}

pub fn insert(conn: &Connection, t: &NewTrigger) -> Result<TriggerRow, InsertErr> {
    let id = uuid::Uuid::new_v4().to_string();
    let approved_at = t.approved_by.map(|_| t.now);
    conn.execute(
        "INSERT INTO source_triggers (id, source_id, on_kind, stale_for_secs, target_project_id, note, \
         cooldown_secs, daily_cap, status, made_by, made_by_project, approved_by, approved_at, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)",
        params![
            id,
            t.source_id,
            t.on.code(),
            t.on.stale_for_secs(),
            t.target_project_id,
            t.note,
            t.cooldown_secs,
            t.daily_cap,
            t.status.code(),
            t.made_by,
            t.made_by_project,
            t.approved_by,
            approved_at,
            t.now
        ],
    )
    .map_err(|e| if is_unique_violation(&e) { InsertErr::Exists } else { InsertErr::Db(e.to_string()) })?;
    get(conn, &id).map_err(|e| InsertErr::Db(e.to_string()))?.ok_or_else(|| InsertErr::Db("row vanished".into()))
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<TriggerRow>> {
    conn.query_row(&format!("SELECT {T_COLUMNS} FROM source_triggers WHERE id = ?1"), params![id], t_row).optional()
}

/// A source's triggers (or every trigger with `None`), oldest first.
pub fn list(conn: &Connection, source_id: Option<&str>) -> rusqlite::Result<Vec<TriggerRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {T_COLUMNS} FROM source_triggers WHERE (?1 IS NULL OR source_id = ?1) ORDER BY created_at, id"
    ))?;
    let rows = stmt.query_map(params![source_id], t_row)?;
    rows.collect()
}

/// Every trigger with this status.
pub fn list_by_status(conn: &Connection, status: TriggerStatus) -> rusqlite::Result<Vec<TriggerRow>> {
    let mut stmt =
        conn.prepare(&format!("SELECT {T_COLUMNS} FROM source_triggers WHERE status = ?1 ORDER BY created_at, id"))?;
    let rows = stmt.query_map(params![status.code()], t_row)?;
    rows.collect()
}

/// Change the editable fields (each `Some` replaces). Returns false when
/// the trigger is gone.
pub struct TriggerEdit<'a> {
    pub on: Option<&'a TriggerOn>,
    pub target_project_id: Option<&'a str>,
    /// `Some(None)` clears the note.
    pub note: Option<Option<&'a str>>,
    pub cooldown_secs: Option<u32>,
    pub daily_cap: Option<u16>,
}

pub fn edit(conn: &Connection, id: &str, e: &TriggerEdit, now: i64) -> Result<bool, InsertErr> {
    let Some(cur) = get(conn, id).map_err(|e| InsertErr::Db(e.to_string()))? else {
        return Ok(false);
    };
    let on = e.on.cloned().unwrap_or(cur.on);
    let note = match e.note {
        Some(n) => n.map(str::to_string),
        None => cur.note,
    };
    conn.execute(
        "UPDATE source_triggers SET on_kind = ?2, stale_for_secs = ?3, target_project_id = ?4, note = ?5, \
         cooldown_secs = ?6, daily_cap = ?7, updated_at = ?8 WHERE id = ?1",
        params![
            id,
            on.code(),
            on.stale_for_secs(),
            e.target_project_id.unwrap_or(&cur.target_project_id),
            note,
            e.cooldown_secs.unwrap_or(cur.cooldown_secs),
            e.daily_cap.unwrap_or(cur.daily_cap),
            now
        ],
    )
    .map_err(|e| if is_unique_violation(&e) { InsertErr::Exists } else { InsertErr::Db(e.to_string()) })?;
    Ok(true)
}

/// Set the status; `approved_by` stamps the approval.
pub fn set_status(
    conn: &Connection,
    id: &str,
    status: TriggerStatus,
    approved_by: Option<&str>,
    now: i64,
) -> rusqlite::Result<bool> {
    let n = match approved_by {
        Some(by) => conn.execute(
            "UPDATE source_triggers SET status = ?2, approved_by = ?3, approved_at = ?4, updated_at = ?4 WHERE id = ?1",
            params![id, status.code(), by, now],
        )?,
        None => conn.execute(
            "UPDATE source_triggers SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.code(), now],
        )?,
    };
    Ok(n > 0)
}

pub fn delete(conn: &Connection, id: &str) -> rusqlite::Result<bool> {
    Ok(conn.execute("DELETE FROM source_triggers WHERE id = ?1", params![id])? > 0)
}

pub fn set_last_fire(conn: &Connection, id: &str, at: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE source_triggers SET last_fire_at = ?2 WHERE id = ?1", params![id, at])?;
    Ok(())
}

/// Mark every active trigger aimed at a project that no longer exists.
pub fn mark_undeliverable_for_project(conn: &Connection, project_id: &str, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE source_triggers SET status = 'undeliverable', updated_at = ?2 \
         WHERE target_project_id = ?1 AND status = 'active'",
        params![project_id, now],
    )
}

// ── fires ────────────────────────────────────────────────────────────

const F_COLUMNS: &str =
    "id, at, incident_id, item_key, outcome, suppress_reason, delivered_at, woke, note_bytes, detail";

fn f_row(r: &Row) -> rusqlite::Result<FireRow> {
    Ok(FireRow {
        id: r.get(0)?,
        at: r.get(1)?,
        incident_id: r.get(2)?,
        item_key: r.get(3)?,
        outcome: r.get(4)?,
        suppress_reason: r.get(5)?,
        delivered_at: r.get(6)?,
        woke: r.get::<_, Option<i64>>(7)?.map(|w| w != 0),
        note_bytes: r.get(8)?,
        detail: r.get(9)?,
    })
}

/// A fire or suppression to log.
pub struct NewFire<'a> {
    pub trigger_id: &'a str,
    pub source_id: &'a str,
    pub target_project_id: &'a str,
    pub incident_id: &'a str,
    pub item_key: Option<&'a str>,
    pub at: i64,
    /// `pending` | `suppressed` | `test`.
    pub outcome: &'a str,
    pub suppress_reason: Option<&'a str>,
}

/// Insert a fire row. `Ok(None)` when the once-per-incident index already
/// holds a live fire for this trigger, incident and item (deduped).
pub fn insert_fire(conn: &Connection, f: &NewFire) -> rusqlite::Result<Option<String>> {
    let id = uuid::Uuid::new_v4().to_string();
    match conn.execute(
        "INSERT INTO source_trigger_fires (id, trigger_id, source_id, target_project_id, incident_id, item_key, at, \
         outcome, suppress_reason) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            f.trigger_id,
            f.source_id,
            f.target_project_id,
            f.incident_id,
            f.item_key,
            f.at,
            f.outcome,
            f.suppress_reason
        ],
    ) {
        Ok(_) => Ok(Some(id)),
        Err(e) if is_unique_violation(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Record how a delivery went.
pub struct Delivery<'a> {
    /// `delivered` | `inbox` | `undeliverable` (or `test` for a test wake).
    pub outcome: &'a str,
    pub delivered_at: i64,
    pub session_id: Option<&'a str>,
    pub woke: Option<bool>,
    pub note_bytes: Option<i64>,
    pub detail: Option<&'a str>,
}

pub fn finish_fire(conn: &Connection, fire_id: &str, d: &Delivery) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE source_trigger_fires SET outcome = ?2, delivered_at = ?3, session_id = ?4, woke = ?5, \
         note_bytes = ?6, detail = ?7 WHERE id = ?1",
        params![fire_id, d.outcome, d.delivered_at, d.session_id, d.woke.map(i64::from), d.note_bytes, d.detail],
    )?;
    Ok(())
}

/// This fire was carried by another fire's note.
pub fn mark_coalesced(conn: &Connection, fire_id: &str, into: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE source_trigger_fires SET outcome = 'coalesced', coalesced_into = ?2 WHERE id = ?1",
        params![fire_id, into],
    )?;
    Ok(())
}

/// A `pending` fire that will never be delivered (its incident closed).
pub fn mark_suppressed(conn: &Connection, fire_id: &str, reason: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE source_trigger_fires SET outcome = 'suppressed', suppress_reason = ?2 WHERE id = ?1",
        params![fire_id, reason],
    )?;
    Ok(())
}

/// Newest first, at most `limit` (<= 200).
pub fn list_fires(conn: &Connection, trigger_id: &str, limit: usize) -> rusqlite::Result<Vec<FireRow>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {F_COLUMNS} FROM source_trigger_fires WHERE trigger_id = ?1 ORDER BY at DESC, rowid DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![trigger_id, limit.min(200) as i64], f_row)?;
    rows.collect()
}

pub fn get_fire(conn: &Connection, fire_id: &str) -> rusqlite::Result<Option<FireRow>> {
    conn.query_row(&format!("SELECT {F_COLUMNS} FROM source_trigger_fires WHERE id = ?1"), params![fire_id], f_row)
        .optional()
}

/// The trigger a fire belongs to, and its source.
pub fn fire_owner(conn: &Connection, fire_id: &str) -> rusqlite::Result<Option<(String, String, String)>> {
    conn.query_row(
        "SELECT trigger_id, source_id, target_project_id FROM source_trigger_fires WHERE id = ?1",
        params![fire_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )
    .optional()
}

const COUNTED: &str = "outcome IN ('pending', 'delivered', 'inbox')";

/// Counted fires of one trigger since `since`.
pub fn fires_since(conn: &Connection, trigger_id: &str, since: i64) -> rusqlite::Result<u16> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM source_trigger_fires WHERE trigger_id = ?1 AND at >= ?2 AND {COUNTED}"),
        params![trigger_id, since],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.clamp(0, i64::from(u16::MAX)) as u16)
}

/// Counted fires to one agent (any trigger) since `since`.
pub fn agent_fires_since(conn: &Connection, project_id: &str, since: i64) -> rusqlite::Result<u16> {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM source_trigger_fires WHERE target_project_id = ?1 AND at >= ?2 AND {COUNTED}"),
        params![project_id, since],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.clamp(0, i64::from(u16::MAX)) as u16)
}

/// Test wakes of one trigger since `since` (<= 5 an hour).
pub fn test_fires_since(conn: &Connection, trigger_id: &str, since: i64) -> rusqlite::Result<u16> {
    conn.query_row(
        "SELECT COUNT(*) FROM source_trigger_fires WHERE trigger_id = ?1 AND at >= ?2 AND outcome = 'test'",
        params![trigger_id, since],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n.clamp(0, i64::from(u16::MAX)) as u16)
}

/// This trigger already has a live fire for this incident (and item).
pub fn incident_fired(conn: &Connection, trigger_id: &str, incident_id: &str, item_key: Option<&str>) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM source_trigger_fires WHERE trigger_id = ?1 AND incident_id = ?2 \
         AND IFNULL(item_key, '') = IFNULL(?3, '') AND outcome IN ('pending', 'delivered', 'inbox', 'coalesced'))",
        params![trigger_id, incident_id, item_key],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n != 0)
}

/// `pending` fires written before `before` (oldest first): `(fire, trigger,
/// incident, item)`.
pub fn pending_before(conn: &Connection, before: i64) -> rusqlite::Result<Vec<(String, String, String, Option<String>)>> {
    let mut stmt = conn.prepare(
        "SELECT id, trigger_id, incident_id, item_key FROM source_trigger_fires \
         WHERE outcome = 'pending' AND at < ?1 ORDER BY at, rowid",
    )?;
    let rows = stmt.query_map(params![before], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
    rows.collect()
}

/// Delete fire rows older than `before` (the 30-day retention; B1's job
/// calls it).
pub fn prune_fires(conn: &Connection, before: i64) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM source_trigger_fires WHERE at < ?1 AND outcome != 'pending'", params![before])
}

// ── audit ────────────────────────────────────────────────────────────

/// One `source_audit` row. `detail` carries names, codes and counts only.
#[allow(clippy::too_many_arguments)]
pub fn audit(
    conn: &Connection,
    at: i64,
    actor: &str,
    actor_kind: &str,
    action: &str,
    source_id: Option<&str>,
    trigger_id: Option<&str>,
    detail: &serde_json::Value,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO source_audit (at, actor, actor_kind, action, source_id, trigger_id, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![at, actor, actor_kind, action, source_id, trigger_id, detail.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, viewers, \
             agent_access, keep, status, made_by, rev, created_at, updated_at) VALUES ('s1', 'fleet', 'Fleet', \
             '{}', ?1, '[]', 60, 'member', 'values', 'values', 'active', 'owner', 1, 0, 0)",
            params!["a".repeat(64)],
        )
        .unwrap();
        conn
    }

    fn new<'a>(on: &'a TriggerOn, status: TriggerStatus) -> NewTrigger<'a> {
        NewTrigger {
            source_id: "s1",
            on,
            target_project_id: "p1",
            note: Some("check the relay"),
            cooldown_secs: 1800,
            daily_cap: 12,
            status,
            made_by: "agent:p1",
            made_by_project: Some("p1"),
            approved_by: None,
            now: 100,
        }
    }

    #[test]
    fn triggers_round_trip_and_the_unique_index_holds() {
        let conn = db();
        let on = TriggerOn::StaleFor { secs: 1800 };
        let t = insert(&conn, &new(&on, TriggerStatus::Proposed)).unwrap();
        assert_eq!(t.on, on);
        assert_eq!(t.status, TriggerStatus::Proposed);
        assert_eq!(t.made_by_project.as_deref(), Some("p1"));
        assert_eq!(insert(&conn, &new(&on, TriggerStatus::Proposed)), Err(InsertErr::Exists));
        let other = TriggerOn::StaleFor { secs: 3600 };
        insert(&conn, &new(&other, TriggerStatus::Active)).unwrap();
        assert_eq!(count_for_source(&conn, "s1").unwrap(), 2);
        assert_eq!(list(&conn, Some("s1")).unwrap().len(), 2);
        assert_eq!(list(&conn, None).unwrap().len(), 2);
        assert_eq!(list_by_status(&conn, TriggerStatus::Active).unwrap().len(), 1);
        // Editing into the other row's slot is refused.
        let e = TriggerEdit { on: Some(&other), target_project_id: None, note: None, cooldown_secs: None, daily_cap: None };
        assert_eq!(edit(&conn, &t.id, &e, 200), Err(InsertErr::Exists));
        let e = TriggerEdit { on: None, target_project_id: None, note: Some(None), cooldown_secs: Some(600), daily_cap: Some(3) };
        assert!(edit(&conn, &t.id, &e, 200).unwrap());
        let t2 = get(&conn, &t.id).unwrap().unwrap();
        assert_eq!((t2.note, t2.cooldown_secs, t2.daily_cap, t2.updated_at), (None, 600, 3, 200));
        assert!(set_status(&conn, &t.id, TriggerStatus::Active, Some("owner"), 300).unwrap());
        let t3 = get(&conn, &t.id).unwrap().unwrap();
        assert_eq!((t3.status, t3.approved_by.as_deref(), t3.approved_at), (TriggerStatus::Active, Some("owner"), Some(300)));
        assert_eq!(mark_undeliverable_for_project(&conn, "p1", 400).unwrap(), 2);
        assert!(delete(&conn, &t.id).unwrap());
        assert_eq!(delete_for_source(&conn, "s1").unwrap(), 1);
    }

    #[test]
    fn fires_dedupe_per_incident_and_count_for_caps() {
        let conn = db();
        let on = TriggerOn::ItemWentCrit;
        let t = insert(&conn, &new(&on, TriggerStatus::Active)).unwrap();
        let fire = |inc: &'static str, item: Option<&'static str>, at: i64, outcome: &'static str| NewFire {
            trigger_id: &t.id,
            source_id: "s1",
            target_project_id: "p1",
            incident_id: inc,
            item_key: item,
            at,
            outcome,
            suppress_reason: (outcome == "suppressed").then_some("cooldown"),
        };
        let f1 = insert_fire(&conn, &fire("i1", Some("ohana"), 1000, "pending")).unwrap().expect("first fire");
        assert_eq!(insert_fire(&conn, &fire("i1", Some("ohana"), 1001, "pending")).unwrap(), None, "deduped");
        assert!(insert_fire(&conn, &fire("i1", Some("luzz"), 1002, "pending")).unwrap().is_some(), "per item");
        assert!(insert_fire(&conn, &fire("i1", Some("ohana"), 1003, "suppressed")).unwrap().is_some(), "suppressions log freely");
        assert!(incident_fired(&conn, &t.id, "i1", Some("ohana")).unwrap());
        assert!(!incident_fired(&conn, &t.id, "i1", None).unwrap());
        assert_eq!(fires_since(&conn, &t.id, 0).unwrap(), 2);
        assert_eq!(agent_fires_since(&conn, "p1", 1002).unwrap(), 1);
        finish_fire(
            &conn,
            &f1,
            &Delivery { outcome: "delivered", delivered_at: 1010, session_id: Some("sess"), woke: Some(true), note_bytes: Some(120), detail: None },
        )
        .unwrap();
        let rows = list_fires(&conn, &t.id, 50).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].at, 1003, "newest first");
        let f1r = get_fire(&conn, &f1).unwrap().unwrap();
        assert_eq!((f1r.outcome.as_str(), f1r.woke, f1r.note_bytes), ("delivered", Some(true), Some(120)));
        assert_eq!(pending_before(&conn, 5000).unwrap().len(), 1, "luzz is still pending");
        let (p, ..) = pending_before(&conn, 5000).unwrap().remove(0);
        mark_coalesced(&conn, &p, &f1).unwrap();
        assert_eq!(get_fire(&conn, &p).unwrap().unwrap().outcome, "coalesced");
        assert!(insert_fire(&conn, &fire("t", None, 1004, "test")).unwrap().is_some());
        assert_eq!(test_fires_since(&conn, &t.id, 0).unwrap(), 1);
        assert_eq!(fires_since(&conn, &t.id, 0).unwrap(), 1, "tests and coalesced rows don't count");
        assert_eq!(fire_owner(&conn, &f1).unwrap().map(|o| o.2), Some("p1".into()));
        assert_eq!(prune_fires(&conn, 2000).unwrap(), 4);
    }

    #[test]
    fn audit_rows_land() {
        let conn = db();
        audit(&conn, 5, "owner", "owner", "trigger_add", Some("s1"), Some("t1"), &serde_json::json!({"on": "went_crit"})).unwrap();
        let (a, d): (String, String) =
            conn.query_row("SELECT action, detail_json FROM source_audit", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(a, "trigger_add");
        assert_eq!(d, r#"{"on":"went_crit"}"#);
    }
}
