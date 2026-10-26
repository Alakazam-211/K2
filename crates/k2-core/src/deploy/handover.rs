//! App handover rows (0226, prd-app-handoff-v1 rev 2): one App's
//! maintenance moving from one workspace (agent) to another on the same
//! box. The daemon (`k2-daemon` `apps::handover`) owns the rules; this is
//! the store.
//!
//! ```text
//! offered ──accept──▶ accepted ──(card)──▶ held ──approve──▶ applying ──▶ applied ──▶ final
//!    │                   │                   │                               └─undo─▶ undone
//!    ├─decline           ├─cancel            ├─deny / stale / expired
//!    ├─cancel            └─expired           └─cancel
//!    └─expired
//! ```
//!
//! An owner-started handover skips `offered`/`accepted` and goes straight
//! to `held` (the card). At most one open handover per App (the partial
//! unique index): a second is `handover_open`.

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

pub const OFFERED: &str = "offered";
pub const ACCEPTED: &str = "accepted";
pub const HELD: &str = "held";
pub const APPLYING: &str = "applying";
pub const APPLIED: &str = "applied";
pub const FINAL: &str = "final";
pub const UNDONE: &str = "undone";
pub const DECLINED: &str = "declined";
pub const CANCELLED: &str = "cancelled";
pub const DENIED: &str = "denied";
pub const STALE: &str = "stale";
pub const EXPIRED: &str = "expired";
pub const ABORTED: &str = "aborted";

/// The open states (the partial unique index's).
pub const OPEN: &[&str] = &[OFFERED, ACCEPTED, HELD, APPLYING];
/// Every state the CHECK allows (a unit test compares them).
pub const ALL: &[&str] = &[OFFERED, ACCEPTED, HELD, APPLYING, APPLIED, FINAL, UNDONE, DECLINED, CANCELLED, DENIED, STALE, EXPIRED, ABORTED];

/// The last apply step (§4.8): commit (2), skin (3), vault (4), runtime
/// (5), re-key started (6), notes (7).
pub const STEPS_ALL: i64 = 7;

/// A handover offered or held with no answer ends after 7 days (the held
/// request's own TTL).
pub const TTL_SECS: i64 = 7 * 24 * 3600;
/// The undo window (Q3: 72 h; at most 7 days).
pub const WINDOW_SECS: i64 = 72 * 3600;
pub const WINDOW_MAX_SECS: i64 = 7 * 24 * 3600;
/// The brief A writes for B (markdown).
pub const BRIEF_MAX: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handover {
    pub id: String,
    pub app_id: String,
    pub service_id: String,
    pub from_workspace: String,
    pub to_workspace: String,
    pub state: String,
    pub started_by: String,
    pub accepted_by: Option<String>,
    pub accepted_at: Option<i64>,
    pub request_id: Option<String>,
    pub approved_by: Option<String>,
    pub approved_at: Option<i64>,
    pub plan: Value,
    pub plan_hash: String,
    pub options: Value,
    #[serde(skip)]
    pub brief: Option<String>,
    pub steps_done: i64,
    pub rekey_state: Option<String>,
    #[serde(skip)]
    pub journal: Option<Value>,
    pub window_until: Option<i64>,
    pub reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Handover {
    pub fn is_open(&self) -> bool {
        OPEN.contains(&self.state.as_str())
    }
    /// Applied and still inside its undo window.
    pub fn in_window(&self, now: i64) -> bool {
        self.state == APPLIED && self.window_until.is_some_and(|w| w > now)
    }
}

const COLS: &str = "id, app_id, service_id, from_workspace, to_workspace, state, started_by, accepted_by, accepted_at, request_id, \
                    approved_by, approved_at, plan_json, plan_hash, options_json, brief, steps_done, rekey_state, journal_json, \
                    window_until, reason, created_at, updated_at";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Handover> {
    let parse = |s: String| serde_json::from_str::<Value>(&s).unwrap_or(Value::Null);
    Ok(Handover {
        id: r.get(0)?,
        app_id: r.get(1)?,
        service_id: r.get(2)?,
        from_workspace: r.get(3)?,
        to_workspace: r.get(4)?,
        state: r.get(5)?,
        started_by: r.get(6)?,
        accepted_by: r.get(7)?,
        accepted_at: r.get(8)?,
        request_id: r.get(9)?,
        approved_by: r.get(10)?,
        approved_at: r.get(11)?,
        plan: parse(r.get(12)?),
        plan_hash: r.get(13)?,
        options: parse(r.get(14)?),
        brief: r.get(15)?,
        steps_done: r.get(16)?,
        rekey_state: r.get(17)?,
        journal: r.get::<_, Option<String>>(18)?.map(parse),
        window_until: r.get(19)?,
        reason: r.get(20)?,
        created_at: r.get(21)?,
        updated_at: r.get(22)?,
    })
}

pub fn new_id() -> String {
    format!("hov_{}", &uuid::Uuid::new_v4().simple().to_string()[..16])
}

/// What a caller starts.
pub struct NewHandover<'a> {
    pub app_id: &'a str,
    pub service_id: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    pub state: &'a str,
    pub started_by: &'a str,
    pub plan: &'a Value,
    pub plan_hash: &'a str,
    pub options: &'a Value,
    pub brief: Option<&'a str>,
    pub reason: Option<&'a str>,
}

/// Insert a new open handover. `Err(Conflict)` when the App already has an
/// open one (the partial unique index).
pub fn insert(conn: &Connection, n: &NewHandover<'_>, now: i64) -> Result<Handover, InsertError> {
    let id = new_id();
    let res = conn.execute(
        "INSERT INTO app_handovers (id, app_id, service_id, from_workspace, to_workspace, state, started_by, plan_json, plan_hash, \
         options_json, brief, reason, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![id, n.app_id, n.service_id, n.from, n.to, n.state, n.started_by, n.plan.to_string(), n.plan_hash, n.options.to_string(), n.brief, n.reason, now],
    );
    match res {
        Ok(_) => {}
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == rusqlite::ErrorCode::ConstraintViolation => return Err(InsertError::Conflict),
        Err(e) => return Err(InsertError::Db(e)),
    }
    get(conn, &id).map_err(InsertError::Db)?.ok_or(InsertError::Db(rusqlite::Error::QueryReturnedNoRows))
}

#[derive(Debug)]
pub enum InsertError {
    /// The App already has an open handover.
    Conflict,
    Db(rusqlite::Error),
}

impl std::fmt::Display for InsertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InsertError::Conflict => write!(f, "the App already has an open handover"),
            InsertError::Db(e) => write!(f, "{e}"),
        }
    }
}

pub fn get(conn: &Connection, id: &str) -> rusqlite::Result<Option<Handover>> {
    conn.query_row(&format!("SELECT {COLS} FROM app_handovers WHERE id = ?1"), params![id], row).optional()
}

/// The App's open handover, if any.
pub fn open_for_app(conn: &Connection, app_id: &str) -> rusqlite::Result<Option<Handover>> {
    conn.query_row(
        &format!("SELECT {COLS} FROM app_handovers WHERE app_id = ?1 AND state IN ('offered','accepted','held','applying')"),
        params![app_id],
        row,
    )
    .optional()
}

/// The App's latest handover (any state).
pub fn latest_for_app(conn: &Connection, app_id: &str) -> rusqlite::Result<Option<Handover>> {
    conn.query_row(&format!("SELECT {COLS} FROM app_handovers WHERE app_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT 1"), params![app_id], row)
        .optional()
}

/// The handover a card (held request) belongs to.
pub fn by_request(conn: &Connection, request_id: &str) -> rusqlite::Result<Option<Handover>> {
    conn.query_row(&format!("SELECT {COLS} FROM app_handovers WHERE request_id = ?1"), params![request_id], row).optional()
}

/// Handovers touching `workspace` (as giver or taker), newest first;
/// `None` = all.
pub fn list(conn: &Connection, workspace: Option<&str>, app_id: Option<&str>, limit: i64) -> rusqlite::Result<Vec<Handover>> {
    let mut st = conn.prepare(&format!(
        "SELECT {COLS} FROM app_handovers WHERE (?1 IS NULL OR from_workspace = ?1 OR to_workspace = ?1) \
         AND (?2 IS NULL OR app_id = ?2) ORDER BY created_at DESC, rowid DESC LIMIT ?3"
    ))?;
    let out = st.query_map(params![workspace, app_id, limit], row)?.collect::<rusqlite::Result<Vec<_>>>();
    out
}

/// Rows in `state` (boot recovery, sweeps).
pub fn in_state(conn: &Connection, state: &str) -> rusqlite::Result<Vec<Handover>> {
    let mut st = conn.prepare(&format!("SELECT {COLS} FROM app_handovers WHERE state = ?1 ORDER BY created_at"))?;
    let out = st.query_map(params![state], row)?.collect::<rusqlite::Result<Vec<_>>>();
    out
}

/// Move `id` from one of `from` to `to`. `Ok(false)` = it wasn't in any of
/// `from` (someone else moved it first).
pub fn transition(conn: &Connection, id: &str, from: &[&str], to: &str, now: i64) -> rusqlite::Result<bool> {
    let list = from.iter().map(|s| format!("'{}'", s.replace('\'', ""))).collect::<Vec<_>>().join(",");
    let n = conn.execute(&format!("UPDATE app_handovers SET state = ?2, updated_at = ?3 WHERE id = ?1 AND state IN ({list})"), params![id, to, now])?;
    Ok(n > 0)
}

pub fn set_accepted(conn: &Connection, id: &str, by: &str, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE app_handovers SET state = 'accepted', accepted_by = ?2, accepted_at = ?3, updated_at = ?3 WHERE id = ?1 AND state = 'offered'",
        params![id, by, now],
    )?;
    Ok(n > 0)
}

/// The card was filed: `held`, with its request id and the plan it shows.
pub fn set_held(conn: &Connection, id: &str, request_id: &str, plan: &Value, plan_hash: &str, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE app_handovers SET state = 'held', request_id = ?2, plan_json = ?3, plan_hash = ?4, updated_at = ?5 \
         WHERE id = ?1 AND state IN ('offered','accepted')",
        params![id, request_id, plan.to_string(), plan_hash, now],
    )?;
    Ok(n > 0)
}

/// The card's held request (an owner-started handover is `held` from the
/// start).
pub fn set_request_id(conn: &Connection, id: &str, request_id: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_handovers SET request_id = ?2, updated_at = ?3 WHERE id = ?1", params![id, request_id, now])?;
    Ok(())
}

pub fn set_approved(conn: &Connection, id: &str, by: &str, now: i64) -> rusqlite::Result<bool> {
    let n = conn.execute(
        "UPDATE app_handovers SET state = 'applying', approved_by = ?2, approved_at = ?3, updated_at = ?3 WHERE id = ?1 AND state = 'held'",
        params![id, by, now],
    )?;
    Ok(n > 0)
}

pub fn set_steps(conn: &Connection, id: &str, steps: i64, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_handovers SET steps_done = MAX(steps_done, ?2), updated_at = ?3 WHERE id = ?1", params![id, steps, now])?;
    Ok(())
}

pub fn set_journal(conn: &Connection, id: &str, journal: Option<&Value>, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_handovers SET journal_json = ?2, updated_at = ?3 WHERE id = ?1", params![id, journal.map(Value::to_string), now])?;
    Ok(())
}

pub fn set_rekey(conn: &Connection, id: &str, state: &str, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_handovers SET rekey_state = ?2, updated_at = ?3 WHERE id = ?1", params![id, state, now])?;
    Ok(())
}

pub fn set_window(conn: &Connection, id: &str, until: Option<i64>, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_handovers SET window_until = ?2, updated_at = ?3 WHERE id = ?1", params![id, until, now])?;
    Ok(())
}

pub fn set_brief(conn: &Connection, id: &str, brief: Option<&str>, now: i64) -> rusqlite::Result<()> {
    conn.execute("UPDATE app_handovers SET brief = ?2, updated_at = ?3 WHERE id = ?1", params![id, brief, now])?;
    Ok(())
}

// ── ref aliases ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefAlias {
    pub service_id: String,
    pub from_workspace: String,
    pub to_workspace: String,
    pub name: String,
    pub to_name: String,
    pub handover_id: String,
}

/// The alias a service's `k2v://ws/<from>/<name>` ref resolves through.
pub fn alias_for(conn: &Connection, service_id: &str, from_workspace: &str, name: &str) -> rusqlite::Result<Option<RefAlias>> {
    conn.query_row(
        "SELECT service_id, from_workspace, to_workspace, name, to_name, handover_id FROM app_ref_aliases \
         WHERE service_id = ?1 AND from_workspace = ?2 AND name = ?3",
        params![service_id, from_workspace, name],
        |r| Ok(RefAlias { service_id: r.get(0)?, from_workspace: r.get(1)?, to_workspace: r.get(2)?, name: r.get(3)?, to_name: r.get(4)?, handover_id: r.get(5)? }),
    )
    .optional()
}

pub fn aliases_of(conn: &Connection, service_id: &str) -> rusqlite::Result<Vec<RefAlias>> {
    let mut st = conn.prepare(
        "SELECT service_id, from_workspace, to_workspace, name, to_name, handover_id FROM app_ref_aliases WHERE service_id = ?1 ORDER BY name",
    )?;
    let out = st
        .query_map(params![service_id], |r| {
            Ok(RefAlias { service_id: r.get(0)?, from_workspace: r.get(1)?, to_workspace: r.get(2)?, name: r.get(3)?, to_name: r.get(4)?, handover_id: r.get(5)? })
        })?
        .collect::<rusqlite::Result<Vec<_>>>();
    out
}

/// Add (or repoint) an alias. Idempotent.
pub fn put_alias(conn: &Connection, a: &RefAlias, now: i64) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO app_ref_aliases (service_id, from_workspace, to_workspace, name, to_name, handover_id, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(service_id, from_workspace, name) DO UPDATE SET \
         to_workspace = excluded.to_workspace, to_name = excluded.to_name, handover_id = excluded.handover_id",
        params![a.service_id, a.from_workspace, a.to_workspace, a.name, a.to_name, a.handover_id, now],
    )?;
    Ok(())
}

/// Remove the aliases one handover made (undo).
pub fn drop_aliases(conn: &Connection, handover_id: &str) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM app_ref_aliases WHERE handover_id = ?1", params![handover_id])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(include_str!("../../drizzle_sql/0226_app_handovers.sql")).unwrap();
        c
    }

    fn new<'a>(app: &'a str, plan: &'a Value, opts: &'a Value) -> NewHandover<'a> {
        NewHandover {
            app_id: app,
            service_id: "svc-1",
            from: "ws-a",
            to: "ws-b",
            state: OFFERED,
            started_by: "agent:a",
            plan,
            plan_hash: "h1",
            options: opts,
            brief: Some("# notes"),
            reason: None,
        }
    }

    #[test]
    fn the_check_names_every_state_and_one_app_has_one_open_handover() {
        let c = conn();
        let sql = include_str!("../../drizzle_sql/0226_app_handovers.sql");
        for s in ALL {
            assert!(sql.contains(&format!("'{s}'")), "0226's CHECK names {s}");
        }
        let (plan, opts) = (serde_json::json!({"app": "jobs"}), serde_json::json!({}));
        let h = insert(&c, &new("jobs", &plan, &opts), 10).unwrap();
        assert!(h.id.starts_with("hov_"), "{}", h.id);
        assert_eq!((h.state.as_str(), h.brief.as_deref()), (OFFERED, Some("# notes")));
        match insert(&c, &new("jobs", &plan, &opts), 11) {
            Err(InsertError::Conflict) => {}
            other => panic!("a second open handover must conflict: {other:?}"),
        }
        // Another App is fine.
        insert(&c, &new("mail", &plan, &opts), 12).unwrap();
        // Closed, a new one may start.
        assert!(transition(&c, &h.id, &[OFFERED], DECLINED, 13).unwrap());
        assert!(!transition(&c, &h.id, &[OFFERED], CANCELLED, 14).unwrap(), "already declined");
        let h2 = insert(&c, &new("jobs", &plan, &opts), 15).unwrap();
        assert!(set_accepted(&c, &h2.id, "agent:b", 16).unwrap());
        assert!(set_held(&c, &h2.id, "w_1", &plan, "h2", 17).unwrap());
        assert_eq!(by_request(&c, "w_1").unwrap().unwrap().id, h2.id);
        assert!(set_approved(&c, &h2.id, "owner", 18).unwrap());
        assert_eq!(open_for_app(&c, "jobs").unwrap().unwrap().state, APPLYING);
        assert!(transition(&c, &h2.id, &[APPLYING], APPLIED, 19).unwrap());
        assert!(open_for_app(&c, "jobs").unwrap().is_none(), "applied isn't open");
        let bad = c.execute("UPDATE app_handovers SET state = 'bogus' WHERE id = ?1", params![h2.id]);
        assert!(bad.is_err(), "the CHECK refuses an unknown state");
    }

    #[test]
    fn aliases_are_per_service_and_go_with_their_handover() {
        let c = conn();
        let a = RefAlias {
            service_id: "svc-1".into(),
            from_workspace: "ws-a".into(),
            to_workspace: "ws-b".into(),
            name: "STRIPE_KEY".into(),
            to_name: "STRIPE_KEY__jobs".into(),
            handover_id: "hov_1".into(),
        };
        put_alias(&c, &a, 1).unwrap();
        put_alias(&c, &a, 2).unwrap();
        assert_eq!(alias_for(&c, "svc-1", "ws-a", "STRIPE_KEY").unwrap().unwrap().to_name, "STRIPE_KEY__jobs");
        assert!(alias_for(&c, "svc-2", "ws-a", "STRIPE_KEY").unwrap().is_none(), "another service has no alias");
        assert_eq!(aliases_of(&c, "svc-1").unwrap().len(), 1);
        assert_eq!(drop_aliases(&c, "hov_1").unwrap(), 1);
        assert!(alias_for(&c, "svc-1", "ws-a", "STRIPE_KEY").unwrap().is_none());
    }
}
