//! rusqlite repo for the 0163 source tables: definitions, state and
//! items, events, history points, audit, the change counter and
//! retention. Every writer bumps `source_rev` and stamps the new value on
//! the row it changed, so `changes?since=` is a plain query.

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::de::DeserializeOwned;

use super::spec::{AgentAccess, Keep, OnTargetError, PauseReason, SimulateState, SourceKind, SourceStatus, Viewers};

/// One `sources` row, typed.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRow {
    pub id: String,
    pub name: String,
    pub label: String,
    pub kind: SourceKind,
    pub preset: Option<String>,
    /// `{fetch, headers, extract, rules}`, canonical JSON.
    pub spec_json: String,
    /// sha256 hex of the spec (`SourceSpec::hash`).
    pub spec_hash: String,
    /// Sorted unique `scheme://host:port`.
    pub origins: Vec<String>,
    /// An exact IP or `http://` target; Owner only.
    pub lan: bool,
    pub secret_name: Option<String>,
    pub url_secret_name: Option<String>,
    /// A vault grant exists now.
    pub secret_bound: bool,
    pub every_secs: u32,
    pub timeout_secs: u32,
    pub max_bytes: u32,
    pub fail_after: u8,
    pub on_target_error: OnTargetError,
    pub link: Option<String>,
    pub viewers: Viewers,
    pub agent_access: AgentAccess,
    pub keep: Keep,
    pub pinned: bool,
    pub pinned_by: Option<String>,
    pub pinned_at: Option<i64>,
    pub status: SourceStatus,
    pub pause_reason: Option<PauseReason>,
    /// `owner` | `login:<user>` | `agent:<projectId>`.
    pub made_by: String,
    pub made_by_project: Option<String>,
    pub approved_by: Option<String>,
    pub approved_at: Option<i64>,
    pub approved_spec_hash: Option<String>,
    pub simulate_state: Option<SimulateState>,
    pub simulate_until: Option<i64>,
    pub simulate_by: Option<String>,
    pub mute_until: Option<i64>,
    pub mute_by: Option<String>,
    pub mute_reason: Option<String>,
    pub rev: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub removed_at: Option<i64>,
    /// The vault scope of the bound secret (0181): `None` = the server's,
    /// `Some(ws)` = that workspace's own secret, bound by its agent.
    pub secret_workspace: Option<String>,
}

const COLUMNS: &str = "id, name, label, kind, preset, spec_json, spec_hash, origins_json, lan, secret_name, \
     url_secret_name, secret_bound, every_secs, timeout_secs, max_bytes, fail_after, on_target_error, link, \
     viewers, agent_access, keep, pinned, pinned_by, pinned_at, status, pause_reason, made_by, made_by_project, \
     approved_by, approved_at, approved_spec_hash, simulate_state, simulate_until, simulate_by, mute_until, \
     mute_by, mute_reason, rev, created_at, updated_at, removed_at, secret_workspace";

fn conv_err(idx: usize, msg: String) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, rusqlite::types::Type::Text, msg.into())
}

/// A stored enum code (`'active'`, `'http-json'`, …) as its serde type.
fn code<T: DeserializeOwned>(r: &Row, idx: usize) -> rusqlite::Result<T> {
    let s: String = r.get(idx)?;
    serde_json::from_value(serde_json::Value::String(s.clone())).map_err(|_| conv_err(idx, format!("unknown code {s:?}")))
}

fn opt_code<T: DeserializeOwned>(r: &Row, idx: usize) -> rusqlite::Result<Option<T>> {
    let s: Option<String> = r.get(idx)?;
    match s {
        None => Ok(None),
        Some(s) => serde_json::from_value(serde_json::Value::String(s.clone()))
            .map(Some)
            .map_err(|_| conv_err(idx, format!("unknown code {s:?}"))),
    }
}

fn row(r: &Row) -> rusqlite::Result<SourceRow> {
    let origins_json: String = r.get(7)?;
    let origins: Vec<String> =
        serde_json::from_str(&origins_json).map_err(|e| conv_err(7, format!("origins_json: {e}")))?;
    Ok(SourceRow {
        id: r.get(0)?,
        name: r.get(1)?,
        label: r.get(2)?,
        kind: code(r, 3)?,
        preset: r.get(4)?,
        spec_json: r.get(5)?,
        spec_hash: r.get(6)?,
        origins,
        lan: r.get::<_, i64>(8)? != 0,
        secret_name: r.get(9)?,
        url_secret_name: r.get(10)?,
        secret_bound: r.get::<_, i64>(11)? != 0,
        every_secs: r.get(12)?,
        timeout_secs: r.get(13)?,
        max_bytes: r.get(14)?,
        fail_after: r.get(15)?,
        on_target_error: code(r, 16)?,
        link: r.get(17)?,
        viewers: code(r, 18)?,
        agent_access: code(r, 19)?,
        keep: code(r, 20)?,
        pinned: r.get::<_, i64>(21)? != 0,
        pinned_by: r.get(22)?,
        pinned_at: r.get(23)?,
        status: code(r, 24)?,
        pause_reason: opt_code(r, 25)?,
        made_by: r.get(26)?,
        made_by_project: r.get(27)?,
        approved_by: r.get(28)?,
        approved_at: r.get(29)?,
        approved_spec_hash: r.get(30)?,
        simulate_state: opt_code(r, 31)?,
        simulate_until: r.get(32)?,
        simulate_by: r.get(33)?,
        mute_until: r.get(34)?,
        mute_by: r.get(35)?,
        mute_reason: r.get(36)?,
        rev: r.get(37)?,
        created_at: r.get(38)?,
        updated_at: r.get(39)?,
        removed_at: r.get(40)?,
        secret_workspace: r.get(41)?,
    })
}

/// The live (not removed) source with this name.
pub fn get_by_name(conn: &Connection, name: &str) -> rusqlite::Result<Option<SourceRow>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM sources WHERE name = ?1 AND status != 'removed'"),
        params![name],
        row,
    )
    .optional()
}

/// Any source row by id, tombstones included.
pub fn get_by_id(conn: &Connection, id: &str) -> rusqlite::Result<Option<SourceRow>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM sources WHERE id = ?1"), params![id], row).optional()
}

/// Every live source, by name.
pub fn list_live(conn: &Connection) -> rusqlite::Result<Vec<SourceRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM sources WHERE status != 'removed' ORDER BY name"))?;
    let rows = stmt.query_map([], row)?;
    rows.collect()
}

/// Bump the one global change counter and return the new value. Call it
/// in the same transaction as the change; the daemon then emits
/// `sources_changed {rev}`.
pub fn bump_rev(conn: &Connection) -> rusqlite::Result<i64> {
    let n = conn.execute("UPDATE source_rev SET rev = rev + 1 WHERE id = 1", [])?;
    if n == 0 {
        conn.execute("INSERT INTO source_rev (id, rev) VALUES (1, 1)", [])?;
    }
    conn.query_row("SELECT rev FROM source_rev WHERE id = 1", [], |r| r.get(0))
}

/// The current change counter.
pub fn current_rev(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT rev FROM source_rev WHERE id = 1", [], |r| r.get(0)).optional().map(|v| v.unwrap_or(0))
}

// ── helpers ──────────────────────────────────────────────────────────

/// A serde enum's wire code (`"active"`, `"http-json"`, …).
pub fn code_of<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

fn opt_code_of<T: serde::Serialize>(v: &Option<T>) -> Option<String> {
    v.as_ref().map(code_of)
}

/// Run `f` in one transaction (commit on Ok, roll back on Err).
pub fn tx<T>(conn: &Connection, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let t = conn.unchecked_transaction()?;
    let out = f(&t)?;
    t.commit()?;
    Ok(out)
}

/// Every source row, tombstones included, by name.
pub fn list_all(conn: &Connection) -> rusqlite::Result<Vec<SourceRow>> {
    let mut stmt = conn.prepare(&format!("SELECT {COLUMNS} FROM sources ORDER BY name, created_at"))?;
    let rows = stmt.query_map([], row)?;
    rows.collect()
}

/// The newest tombstone for `name` that still holds it (removed less than
/// `TOMBSTONE_SECS` ago): its `removed_at`.
pub fn held_name(conn: &Connection, name: &str, now: i64) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT MAX(removed_at) FROM sources WHERE name = ?1 AND status = 'removed' AND removed_at > ?2",
        params![name, now - super::TOMBSTONE_SECS],
        |r| r.get::<_, Option<i64>>(0),
    )
}

/// Live sources and their target count (for the per-server caps).
pub fn live_counts(conn: &Connection) -> rusqlite::Result<(usize, usize)> {
    let mut n = 0usize;
    let mut targets = 0usize;
    for r in list_live(conn)? {
        n += 1;
        targets += target_count(&r);
    }
    Ok((n, targets))
}

/// How many targets a row fetches (1 for a single URL).
pub fn target_count(r: &SourceRow) -> usize {
    serde_json::from_str::<serde_json::Value>(&r.spec_json)
        .ok()
        .and_then(|v| v.pointer("/fetch/targets").and_then(|t| t.as_array().map(Vec::len)))
        .unwrap_or(1)
}

/// The definition a row holds, as a spec (name/label/columns filled).
pub fn spec_of(r: &SourceRow) -> Result<super::spec::SourceSpec, String> {
    let mut v: serde_json::Value = serde_json::from_str(&r.spec_json).map_err(|e| e.to_string())?;
    let m = v.as_object_mut().ok_or("spec_json isn't an object")?;
    m.insert("name".into(), r.name.clone().into());
    m.insert("label".into(), r.label.clone().into());
    m.insert("kind".into(), code_of(&r.kind).into());
    m.insert("preset".into(), r.preset.clone().into());
    m.insert("secret".into(), r.secret_name.clone().into());
    m.insert("everySecs".into(), r.every_secs.into());
    m.insert("timeoutSecs".into(), r.timeout_secs.into());
    m.insert("maxBytes".into(), r.max_bytes.into());
    m.insert("failAfter".into(), r.fail_after.into());
    m.insert("onTargetError".into(), code_of(&r.on_target_error).into());
    m.insert("link".into(), r.link.clone().into());
    m.insert("agentAccess".into(), code_of(&r.agent_access).into());
    m.insert("viewers".into(), code_of(&r.viewers).into());
    m.insert("keep".into(), code_of(&r.keep).into());
    serde_json::from_value(v).map_err(|e| e.to_string())
}

// ── writes ───────────────────────────────────────────────────────────

/// A new source from a validated spec. `status` is `proposed` or `active`.
pub struct NewSource<'a> {
    pub v: &'a super::spec::Validated,
    pub status: SourceStatus,
    /// `owner` | `login:<u>` | `agent:<projectId>`.
    pub made_by: &'a str,
    pub made_by_project: Option<&'a str>,
    /// Set when the source starts active (a person added it).
    pub approved_by: Option<&'a str>,
    pub now: i64,
}

/// Insert a source and its pending state row. Returns the row.
pub fn insert_source(conn: &Connection, n: &NewSource) -> rusqlite::Result<SourceRow> {
    let s = &n.v.spec;
    let id = uuid::Uuid::new_v4().to_string();
    let rev = bump_rev(conn)?;
    let hash = s.hash();
    conn.execute(
        "INSERT INTO sources (id, name, label, kind, preset, spec_json, spec_hash, origins_json, lan, secret_name, \
         url_secret_name, secret_bound, every_secs, timeout_secs, max_bytes, fail_after, on_target_error, link, viewers, \
         agent_access, keep, status, made_by, made_by_project, approved_by, approved_at, approved_spec_hash, rev, \
         created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12, ?13, ?14, ?15, ?16, ?17, \
         ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?28)",
        params![
            id,
            s.name,
            s.label,
            code_of(&s.kind),
            s.preset,
            s.stored_json(),
            hash,
            serde_json::to_string(&n.v.origins).unwrap_or_else(|_| "[]".into()),
            n.v.lan as i64,
            s.secret,
            s.url_secret_name(),
            s.every_secs,
            s.timeout_secs,
            s.max_bytes,
            s.fail_after,
            code_of(&s.on_target_error),
            s.link,
            opt_code_of(&s.viewers),
            opt_code_of(&s.agent_access),
            code_of(&s.keep),
            code_of(&n.status),
            n.made_by,
            n.made_by_project,
            n.approved_by,
            n.approved_by.map(|_| n.now),
            n.approved_by.map(|_| hash.clone()),
            rev,
            n.now,
        ],
    )?;
    conn.execute(
        "INSERT INTO source_state (source_id, rev) VALUES (?1, ?2)",
        params![id, rev],
    )?;
    get_by_id(conn, &id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)
}

/// Replace a source's definition (an edit). `status` is the status after
/// the edit; `unbind` clears `secret_bound`. Bumps `rev`.
pub fn update_spec(
    conn: &Connection,
    id: &str,
    v: &super::spec::Validated,
    status: SourceStatus,
    unbind: bool,
    now: i64,
) -> rusqlite::Result<i64> {
    let s = &v.spec;
    let rev = bump_rev(conn)?;
    conn.execute(
        "UPDATE sources SET label = ?2, preset = ?3, spec_json = ?4, spec_hash = ?5, origins_json = ?6, lan = ?7, \
         secret_name = ?8, url_secret_name = ?9, secret_bound = CASE WHEN ?10 THEN 0 ELSE secret_bound END, \
         secret_workspace = CASE WHEN ?10 THEN NULL ELSE secret_workspace END, \
         every_secs = ?11, timeout_secs = ?12, max_bytes = ?13, fail_after = ?14, on_target_error = ?15, link = ?16, \
         viewers = ?17, agent_access = ?18, keep = ?19, status = ?20, rev = ?21, updated_at = ?22 WHERE id = ?1",
        params![
            id,
            s.label,
            s.preset,
            s.stored_json(),
            s.hash(),
            serde_json::to_string(&v.origins).unwrap_or_else(|_| "[]".into()),
            v.lan as i64,
            s.secret,
            s.url_secret_name(),
            unbind,
            s.every_secs,
            s.timeout_secs,
            s.max_bytes,
            s.fail_after,
            code_of(&s.on_target_error),
            s.link,
            opt_code_of(&s.viewers),
            opt_code_of(&s.agent_access),
            code_of(&s.keep),
            code_of(&status),
            rev,
            now,
        ],
    )?;
    Ok(rev)
}

/// Set status (and pause reason). Bumps `rev`.
pub fn set_status(
    conn: &Connection,
    id: &str,
    status: SourceStatus,
    pause_reason: Option<super::spec::PauseReason>,
    now: i64,
) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    conn.execute(
        "UPDATE sources SET status = ?2, pause_reason = ?3, rev = ?4, updated_at = ?5 WHERE id = ?1",
        params![id, code_of(&status), opt_code_of(&pause_reason), rev, now],
    )?;
    Ok(rev)
}

/// Approve: active, who/when/what hash. Bumps `rev`.
pub fn mark_approved(conn: &Connection, id: &str, by: &str, spec_hash: &str, now: i64) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    conn.execute(
        "UPDATE sources SET status = 'active', pause_reason = NULL, approved_by = ?2, approved_at = ?3, \
         approved_spec_hash = ?4, rev = ?5, updated_at = ?3 WHERE id = ?1",
        params![id, by, now, spec_hash, rev],
    )?;
    Ok(rev)
}

/// `secret_bound` (after a vault grant is made or revoked). Bumps `rev`.
pub fn set_secret_bound(conn: &Connection, id: &str, bound: bool, now: i64) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    conn.execute(
        "UPDATE sources SET secret_bound = ?2, rev = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, bound as i64, rev, now],
    )?;
    Ok(rev)
}

/// Mute until `until` (None clears). Bumps `rev`.
pub fn set_mute(
    conn: &Connection,
    id: &str,
    until: Option<i64>,
    by: Option<&str>,
    reason: Option<&str>,
    now: i64,
) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    conn.execute(
        "UPDATE sources SET mute_until = ?2, mute_by = ?3, mute_reason = ?4, rev = ?5, updated_at = ?6 WHERE id = ?1",
        params![id, until, until.and(by), until.and(reason), rev, now],
    )?;
    Ok(rev)
}

/// Simulate a state until `until` (None clears). Bumps `rev`.
pub fn set_simulate(
    conn: &Connection,
    id: &str,
    state: Option<SimulateState>,
    until: Option<i64>,
    by: Option<&str>,
    now: i64,
) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    let (state, until) = match (state, until) {
        (Some(s), Some(u)) => (Some(code_of(&s)), Some(u)),
        _ => (None, None),
    };
    conn.execute(
        "UPDATE sources SET simulate_state = ?2, simulate_until = ?3, simulate_by = ?4, rev = ?5, updated_at = ?6 \
         WHERE id = ?1",
        params![id, state, until, until.and(by), rev, now],
    )?;
    Ok(rev)
}

/// Pin or unpin. Bumps `rev`.
pub fn set_pin(conn: &Connection, id: &str, pinned: bool, by: &str, now: i64) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    conn.execute(
        "UPDATE sources SET pinned = ?2, pinned_by = ?3, pinned_at = ?4, rev = ?5, updated_at = ?6 WHERE id = ?1",
        params![id, pinned as i64, pinned.then_some(by), pinned.then_some(now), rev, now],
    )?;
    Ok(rev)
}

/// Bump the source's `rev` without changing it (e.g. a trigger fired).
pub fn touch(conn: &Connection, id: &str) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    conn.execute("UPDATE sources SET rev = ?2 WHERE id = ?1", params![id, rev])?;
    Ok(rev)
}

/// Tombstone a source and delete its triggers (one transaction is the
/// caller's). Returns `(rev, triggers removed)`.
pub fn tombstone(conn: &Connection, id: &str, now: i64) -> rusqlite::Result<(i64, usize)> {
    let rev = bump_rev(conn)?;
    let removed = super::trigger_store::delete_for_source(conn, id)?;
    conn.execute(
        "UPDATE sources SET status = 'removed', removed_at = ?2, pinned = 0, simulate_state = NULL, \
         simulate_until = NULL, simulate_by = NULL, mute_until = NULL, mute_by = NULL, mute_reason = NULL, \
         secret_bound = 0, rev = ?3, updated_at = ?2 WHERE id = ?1",
        params![id, now, rev],
    )?;
    Ok((rev, removed))
}

/// Delete tombstones older than `TOMBSTONE_SECS` and every child row (no
/// reliance on cascade). Returns how many sources went.
pub fn reap_tombstones(conn: &Connection, now: i64) -> rusqlite::Result<usize> {
    let ids: Vec<(String, i64)> = {
        let mut stmt =
            conn.prepare("SELECT id, rev FROM sources WHERE status = 'removed' AND removed_at <= ?1")?;
        let rows = stmt.query_map(params![now - super::TOMBSTONE_SECS], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    if let Some(max) = ids.iter().map(|(_, r)| *r).max() {
        conn.execute("UPDATE source_rev SET reaped_rev = MAX(reaped_rev, ?1) WHERE id = 1", params![max])?;
    }
    for (id, _) in &ids {
        for t in [
            "DELETE FROM source_items WHERE source_id = ?1",
            "DELETE FROM source_state WHERE source_id = ?1",
            "DELETE FROM source_points WHERE source_id = ?1",
            "DELETE FROM source_events WHERE source_id = ?1",
            "DELETE FROM source_trigger_fires WHERE source_id = ?1",
            "DELETE FROM source_triggers WHERE source_id = ?1",
            "DELETE FROM sources WHERE id = ?1",
        ] {
            conn.execute(t, params![id])?;
        }
    }
    Ok(ids.len())
}

// ── state ────────────────────────────────────────────────────────────

use super::extract::Projection;
use super::state::{Counts, GapKind, Health, ItemState, Phase, Reason, SourceState, Transition};

fn reason_of(s: Option<String>) -> Option<Reason> {
    s.and_then(|c| Reason::from_code(&c))
}

fn health_of(s: &str) -> Health {
    serde_json::from_value(serde_json::Value::String(s.to_string())).unwrap_or(Health::Unknown)
}

/// The state row, its items and the stored projection.
pub fn load_state(conn: &Connection, id: &str) -> rusqlite::Result<Option<(SourceState, Option<Projection>)>> {
    let got = conn
        .query_row(
            "SELECT health, phase, reason, http_status, soft_health, soft_count, self_health, fetch_failures, backoff_k, \
             hard_since, recovered_at, updated_at, checked_at, next_at, gap_kind, gap_from, gap_to, incident_id, \
             projection_json, counts_json, etag, last_modified, vault_denied, rev FROM source_state WHERE source_id = ?1",
            params![id],
            |r| {
                let health: String = r.get(0)?;
                let phase: String = r.get(1)?;
                let soft: Option<String> = r.get(4)?;
                let selfh: Option<String> = r.get(6)?;
                let gap: Option<String> = r.get(14)?;
                let proj: Option<String> = r.get(18)?;
                let counts: String = r.get(19)?;
                let st = SourceState {
                    source_id: id.to_string(),
                    health: health_of(&health),
                    phase: serde_json::from_value(serde_json::Value::String(phase)).unwrap_or(Phase::Pending),
                    reason: reason_of(r.get(2)?),
                    http_status: r.get(3)?,
                    soft_health: soft.as_deref().map(health_of),
                    soft_count: r.get(5)?,
                    self_health: selfh.as_deref().map(health_of),
                    fetch_failures: r.get(7)?,
                    backoff_k: r.get(8)?,
                    hard_since: r.get(9)?,
                    recovered_at: r.get(10)?,
                    updated_at: r.get(11)?,
                    checked_at: r.get(12)?,
                    next_at: r.get(13)?,
                    gap_kind: gap.and_then(|g| serde_json::from_value::<GapKind>(serde_json::Value::String(g)).ok()),
                    gap_from: r.get(15)?,
                    gap_to: r.get(16)?,
                    incident_id: r.get(17)?,
                    counts: serde_json::from_str::<Counts>(&counts).unwrap_or_default(),
                    etag: r.get(20)?,
                    last_modified: r.get(21)?,
                    vault_denied: r.get::<_, i64>(22)? != 0,
                    rev: r.get(23)?,
                    items: Default::default(),
                };
                let p = proj.and_then(|s| serde_json::from_str::<Projection>(&s).ok());
                Ok((st, p))
            },
        )
        .optional()?;
    let Some((mut st, p)) = got else { return Ok(None) };
    let mut stmt = conn.prepare(
        "SELECT item_key, is_target, origin, health, soft_health, soft_count, reason, http_status, hard_since, \
         recovered_at, updated_at, checked_at, etag, last_modified, incident_id FROM source_items WHERE source_id = ?1",
    )?;
    let items = stmt.query_map(params![id], |r| {
        let h: String = r.get(3)?;
        let soft: Option<String> = r.get(4)?;
        Ok(ItemState {
            item_key: r.get(0)?,
            is_target: r.get::<_, i64>(1)? != 0,
            origin: r.get(2)?,
            health: health_of(&h),
            soft_health: soft.as_deref().map(health_of),
            soft_count: r.get(5)?,
            reason: reason_of(r.get(6)?),
            http_status: r.get(7)?,
            hard_since: r.get(8)?,
            recovered_at: r.get(9)?,
            updated_at: r.get(10)?,
            checked_at: r.get(11)?,
            etag: r.get(12)?,
            last_modified: r.get(13)?,
            incident_id: r.get(14)?,
        })
    })?;
    for it in items {
        let it = it?;
        st.items.insert(it.item_key.clone(), it);
    }
    Ok(Some((st, p)))
}

/// Write the state row (and items). `projection = None` keeps the stored
/// one. Bumps `rev` and stamps it on the row; returns it.
pub fn save_state(conn: &Connection, st: &SourceState, projection: Option<&Projection>) -> rusqlite::Result<i64> {
    let rev = bump_rev(conn)?;
    let code = |h: Health| h.code();
    conn.execute(
        "UPDATE source_state SET health = ?2, phase = ?3, reason = ?4, http_status = ?5, soft_health = ?6, \
         soft_count = ?7, self_health = ?8, fetch_failures = ?9, backoff_k = ?10, hard_since = ?11, recovered_at = ?12, \
         updated_at = ?13, checked_at = ?14, next_at = ?15, gap_kind = ?16, gap_from = ?17, gap_to = ?18, \
         incident_id = ?19, projection_json = COALESCE(?20, projection_json), counts_json = ?21, etag = ?22, \
         last_modified = ?23, vault_denied = ?24, rev = ?25 WHERE source_id = ?1",
        params![
            st.source_id,
            code(st.health),
            st.phase.code(),
            st.reason.map(Reason::code),
            st.http_status,
            st.soft_health.map(code),
            st.soft_count,
            st.self_health.map(code),
            st.fetch_failures,
            st.backoff_k,
            st.hard_since,
            st.recovered_at,
            st.updated_at,
            st.checked_at,
            st.next_at,
            st.gap_kind.map(GapKind::code),
            st.gap_from,
            st.gap_to,
            st.incident_id,
            projection.map(Projection::to_stored_json),
            serde_json::to_string(&st.counts).unwrap_or_else(|_| "{}".into()),
            st.etag,
            st.last_modified,
            st.vault_denied as i64,
            rev,
        ],
    )?;
    let keys: Vec<&str> = st.items.keys().map(String::as_str).collect();
    let existing: Vec<String> = {
        let mut stmt = conn.prepare("SELECT item_key FROM source_items WHERE source_id = ?1")?;
        let rows = stmt.query_map(params![st.source_id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for k in existing.iter().filter(|k| !keys.contains(&k.as_str())) {
        conn.execute("DELETE FROM source_items WHERE source_id = ?1 AND item_key = ?2", params![st.source_id, k])?;
    }
    for it in st.items.values() {
        conn.execute(
            "INSERT INTO source_items (source_id, item_key, is_target, origin, health, soft_health, soft_count, reason, \
             http_status, hard_since, recovered_at, updated_at, checked_at, etag, last_modified, incident_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16) \
             ON CONFLICT(source_id, item_key) DO UPDATE SET is_target = excluded.is_target, origin = excluded.origin, \
             health = excluded.health, soft_health = excluded.soft_health, soft_count = excluded.soft_count, \
             reason = excluded.reason, http_status = excluded.http_status, hard_since = excluded.hard_since, \
             recovered_at = excluded.recovered_at, updated_at = excluded.updated_at, checked_at = excluded.checked_at, \
             etag = excluded.etag, last_modified = excluded.last_modified, incident_id = excluded.incident_id",
            params![
                st.source_id,
                it.item_key,
                it.is_target as i64,
                it.origin,
                code(it.health),
                it.soft_health.map(code),
                it.soft_count,
                it.reason.map(Reason::code),
                it.http_status,
                it.hard_since,
                it.recovered_at,
                it.updated_at,
                it.checked_at,
                it.etag,
                it.last_modified,
                it.incident_id,
            ],
        )?;
    }
    Ok(rev)
}

/// Append transitions to the event list, keeping the newest 200.
pub fn insert_events(conn: &Connection, source_id: &str, at: i64, ts: &[Transition], simulated: bool) -> rusqlite::Result<()> {
    for t in ts {
        conn.execute(
            "INSERT INTO source_events (source_id, at, scope, item_key, kind, from_health, to_health, reason, \
             incident_id, simulated) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                source_id,
                at,
                if t.scope_item.is_some() { "item" } else { "source" },
                t.scope_item,
                t.kind.code(),
                t.from.code(),
                t.to.code(),
                t.reason.map(Reason::code),
                t.incident_id,
                simulated as i64,
            ],
        )?;
    }
    conn.execute(
        "DELETE FROM source_events WHERE source_id = ?1 AND id NOT IN \
         (SELECT id FROM source_events WHERE source_id = ?1 ORDER BY id DESC LIMIT 200)",
        params![source_id],
    )?;
    Ok(())
}

/// One recorded event (`source_events`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub at: i64,
    pub scope: String,
    pub item_key: Option<String>,
    pub kind: String,
    pub from_health: Option<String>,
    pub to_health: Option<String>,
    pub reason: Option<String>,
    pub incident_id: Option<String>,
}

pub fn events(conn: &Connection, source_id: &str, limit: usize) -> rusqlite::Result<Vec<EventRow>> {
    let mut stmt = conn.prepare(
        "SELECT at, scope, item_key, kind, from_health, to_health, reason, incident_id FROM source_events \
         WHERE source_id = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![source_id, limit as i64], |r| {
        Ok(EventRow {
            at: r.get(0)?,
            scope: r.get(1)?,
            item_key: r.get(2)?,
            kind: r.get(3)?,
            from_health: r.get(4)?,
            to_health: r.get(5)?,
            reason: r.get(6)?,
            incident_id: r.get(7)?,
        })
    })?;
    rows.collect()
}

// ── history ──────────────────────────────────────────────────────────

/// Raw points at `t` (`v = None` is a gap marker).
pub fn insert_points(conn: &Connection, source_id: &str, t: i64, pts: &[(String, Option<f64>)]) -> rusqlite::Result<()> {
    for (series, v) in pts {
        conn.execute(
            "INSERT OR REPLACE INTO source_points (source_id, series, res, t, v) VALUES (?1, ?2, 'raw', ?3, ?4)",
            params![source_id, series, t, v],
        )?;
    }
    Ok(())
}

/// Points of one series at one resolution in `[from, to]`, by time.
pub fn points(conn: &Connection, source_id: &str, series: &str, res: &str, from: i64, to: i64) -> rusqlite::Result<Vec<(i64, Option<f64>)>> {
    let mut stmt = conn.prepare(
        "SELECT t, v FROM source_points WHERE source_id = ?1 AND series = ?2 AND res = ?3 AND t >= ?4 AND t <= ?5 ORDER BY t",
    )?;
    let rows = stmt.query_map(params![source_id, series, res, from, to], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// Hourly retention (contract §2): roll raw points older than 48 h into
/// hourly means, then drop them; drop hourly points older than 35 days;
/// events to 200 per source; fires older than 30 days; audit older than
/// 90 days; reap tombstones older than 7 days.
pub fn prune(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    let cutoff = now - super::history::RAW_KEEP_SECS;
    let series: Vec<(String, String)> = {
        let mut stmt = conn.prepare("SELECT DISTINCT source_id, series FROM source_points WHERE res = 'raw' AND t < ?1")?;
        let rows = stmt.query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    for (sid, s) in &series {
        let raw = points(conn, sid, s, "raw", i64::MIN, cutoff - 1)?;
        for (h, v) in super::history::hourly_means(&raw, cutoff) {
            conn.execute(
                "INSERT OR REPLACE INTO source_points (source_id, series, res, t, v) VALUES (?1, ?2, 'hour', ?3, ?4)",
                params![sid, s, h, v],
            )?;
        }
    }
    conn.execute("DELETE FROM source_points WHERE res = 'raw' AND t < ?1", params![cutoff])?;
    conn.execute(
        "DELETE FROM source_points WHERE res = 'hour' AND t < ?1",
        params![now - super::history::HOUR_KEEP_SECS],
    )?;
    conn.execute("DELETE FROM source_trigger_fires WHERE at < ?1", params![now - 30 * 86400])?;
    conn.execute("DELETE FROM source_audit WHERE at < ?1", params![now - 90 * 86400])?;
    reap_tombstones(conn, now)?;
    Ok(())
}

// ── audit ────────────────────────────────────────────────────────────

/// One audit line. `detail` holds names, codes and counts only.
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

/// Whether `actor` already has an `action` line for `source_id` since `since`.
pub fn audited_since(conn: &Connection, actor: &str, action: &str, source_id: &str, since: i64) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT COUNT(*) FROM source_audit WHERE actor = ?1 AND action = ?2 AND source_id = ?3 AND at >= ?4",
        params![actor, action, source_id, since],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
}

// ── changes ──────────────────────────────────────────────────────────

/// Sources whose definition or state changed after `since` (live rows),
/// and names removed after `since`.
pub fn changed_since(conn: &Connection, since: i64) -> rusqlite::Result<(Vec<SourceRow>, Vec<String>)> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {} FROM sources s LEFT JOIN source_state st ON st.source_id = s.id \
         WHERE s.status != 'removed' AND (s.rev > ?1 OR IFNULL(st.rev, 0) > ?1) ORDER BY s.name",
        COLUMNS.split(", ").map(|c| format!("s.{}", c.trim())).collect::<Vec<_>>().join(", ")
    ))?;
    let changed = stmt.query_map(params![since], row)?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut stmt = conn.prepare("SELECT name FROM sources WHERE status = 'removed' AND rev > ?1 ORDER BY name")?;
    let removed = stmt.query_map(params![since], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok((changed, removed))
}

/// The highest `rev` of a reaped tombstone: a `changes?since=` older than
/// it may miss a removal, so the answer is `full`.
pub fn reaped_rev(conn: &Connection) -> rusqlite::Result<i64> {
    conn.query_row("SELECT reaped_rev FROM source_rev WHERE id = 1", [], |r| r.get(0)).optional().map(|v| v.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn rows_read_back_typed_and_rev_counts_up() {
        let conn = db();
        assert_eq!(current_rev(&conn).unwrap(), 0);
        conn.execute(
            "INSERT INTO sources (id, name, label, spec_json, spec_hash, origins_json, every_secs, viewers, \
             agent_access, keep, status, pause_reason, made_by, made_by_project, simulate_state, simulate_until, \
             rev, created_at, updated_at) VALUES ('s1', 'fleet', 'Fleet', '{}', ?1, \
             '[\"https://acme.k2.dev:443\"]', 60, 'admin', 'shape', 'states', 'paused', 'paused_by_person', \
             'agent:p1', 'p1', 'crit', 1760000300, 1, 1760000000, 1760000000)",
            params!["b".repeat(64)],
        )
        .unwrap();
        let r = get_by_name(&conn, "fleet").unwrap().expect("live row");
        assert_eq!(r.kind, SourceKind::HttpJson);
        assert_eq!(r.origins, vec!["https://acme.k2.dev:443".to_string()]);
        assert_eq!((r.viewers, r.agent_access, r.keep), (Viewers::Admin, AgentAccess::Shape, Keep::States));
        assert_eq!((r.status, r.pause_reason), (SourceStatus::Paused, Some(PauseReason::PausedByPerson)));
        assert_eq!(r.simulate_state, Some(SimulateState::Crit));
        assert_eq!(r.made_by_project.as_deref(), Some("p1"));
        assert_eq!(get_by_id(&conn, "s1").unwrap().map(|r| r.name), Some("fleet".into()));
        assert_eq!(list_live(&conn).unwrap().len(), 1);
        assert_eq!(bump_rev(&conn).unwrap(), 1);
        assert_eq!(bump_rev(&conn).unwrap(), 2);
        assert_eq!(current_rev(&conn).unwrap(), 2);
    }

    fn validated(name: &str, n_targets: usize) -> crate::sources::spec::Validated {
        let targets: Vec<_> = (0..n_targets)
            .map(|i| serde_json::json!({"key": format!("b{i}"), "url": format!("https://b{i}.example.com/boot-status")}))
            .collect();
        let s: crate::sources::spec::SourceSpec = serde_json::from_value(serde_json::json!({
            "name": name, "label": "Fleet", "fetch": {"targets": targets}, "everySecs": 60,
            "extract": {"items": {"itemFields": {"version": "/version"}}, "aggregates": {"n": {"op": "count"}}, "trend": ["n"]}
        }))
        .unwrap();
        s.validate(false).unwrap()
    }

    #[test]
    fn insert_spec_round_trip_state_save_and_changes() {
        let conn = db();
        let v = validated("fleet", 3);
        let row = insert_source(
            &conn,
            &NewSource { v: &v, status: SourceStatus::Proposed, made_by: "agent:p1", made_by_project: Some("p1"), approved_by: None, now: 100 },
        )
        .unwrap();
        assert_eq!(row.status, SourceStatus::Proposed);
        assert_eq!(row.spec_hash, v.spec.hash());
        assert_eq!(target_count(&row), 3);
        assert_eq!(live_counts(&conn).unwrap(), (1, 3));
        let back = spec_of(&row).unwrap();
        assert_eq!(back.hash(), v.spec.hash(), "a stored spec reads back to the same hash");
        assert_eq!(back, v.spec);

        let (mut st, p) = load_state(&conn, &row.id).unwrap().expect("state row");
        assert_eq!((st.phase, p), (Phase::Pending, None));
        st.phase = Phase::Live;
        st.health = Health::Crit;
        st.self_health = Some(Health::Ok);
        st.items.insert("b0".into(), ItemState::new("b0", true));
        st.items.get_mut("b0").unwrap().reason = Some(Reason::Timeout);
        st.items.get_mut("b0").unwrap().health = Health::Crit;
        let proj = Projection { fields: [("n".to_string(), serde_json::json!(3))].into(), items: vec![] };
        let before = current_rev(&conn).unwrap();
        let rev = save_state(&conn, &st, Some(&proj)).unwrap();
        assert!(rev > before);
        let (st2, p2) = load_state(&conn, &row.id).unwrap().unwrap();
        assert_eq!(st2.items["b0"].reason, Some(Reason::Timeout));
        assert_eq!((st2.health, st2.self_health), (Health::Crit, Some(Health::Ok)));
        assert_eq!(p2.unwrap().fields["n"], serde_json::json!(3));
        // None keeps the projection; dropped items are deleted.
        let mut st3 = st2.clone();
        st3.items.clear();
        save_state(&conn, &st3, None).unwrap();
        let (st4, p4) = load_state(&conn, &row.id).unwrap().unwrap();
        assert!(st4.items.is_empty());
        assert!(p4.is_some());

        let (changed, removed) = changed_since(&conn, before).unwrap();
        assert_eq!(changed.len(), 1);
        assert!(removed.is_empty());
        let now_rev = current_rev(&conn).unwrap();
        assert!(changed_since(&conn, now_rev).unwrap().0.is_empty());
    }

    #[test]
    fn tombstones_hold_the_name_seven_days_and_reaping_raises_reaped_rev() {
        let conn = db();
        let v = validated("fleet", 1);
        let row = insert_source(&conn, &NewSource { v: &v, status: SourceStatus::Active, made_by: "owner", made_by_project: None, approved_by: Some("owner"), now: 100 }).unwrap();
        assert_eq!(row.approved_by.as_deref(), Some("owner"));
        conn.execute(
            "INSERT INTO source_triggers (id, source_id, on_kind, target_project_id, status, made_by, created_at, updated_at) \
             VALUES ('t1', ?1, 'went_crit', 'p1', 'active', 'owner', 100, 100)",
            params![row.id],
        )
        .unwrap();
        let before = current_rev(&conn).unwrap();
        let (_, triggers) = tombstone(&conn, &row.id, 1_000).unwrap();
        assert_eq!(triggers, 1);
        assert!(get_by_name(&conn, "fleet").unwrap().is_none());
        assert_eq!(held_name(&conn, "fleet", 1_000 + 3600).unwrap(), Some(1_000));
        assert_eq!(held_name(&conn, "fleet", 1_000 + crate::sources::TOMBSTONE_SECS + 1).unwrap(), None);
        let (_, removed) = changed_since(&conn, before).unwrap();
        assert_eq!(removed, vec!["fleet".to_string()]);
        // A new live source may take the name while the tombstone exists.
        insert_source(&conn, &NewSource { v: &v, status: SourceStatus::Proposed, made_by: "owner", made_by_project: None, approved_by: None, now: 2_000 }).unwrap();
        assert_eq!(reaped_rev(&conn).unwrap(), 0);
        prune(&conn, 1_000 + crate::sources::TOMBSTONE_SECS + 1).unwrap();
        assert!(get_by_id(&conn, &row.id).unwrap().is_none(), "reaped");
        assert!(reaped_rev(&conn).unwrap() > before);
        assert_eq!(list_all(&conn).unwrap().len(), 1);
    }

    #[test]
    fn points_roll_up_into_hours_and_events_keep_200() {
        let conn = db();
        let v = validated("fleet", 1);
        let row = insert_source(&conn, &NewSource { v: &v, status: SourceStatus::Active, made_by: "owner", made_by_project: None, approved_by: Some("owner"), now: 0 }).unwrap();
        let now = 10 * 86400;
        let old = now - crate::sources::history::RAW_KEEP_SECS - 7200;
        insert_points(&conn, &row.id, old, &[("n".into(), Some(2.0))]).unwrap();
        insert_points(&conn, &row.id, old + 60, &[("n".into(), Some(4.0))]).unwrap();
        insert_points(&conn, &row.id, now - 60, &[("n".into(), None)]).unwrap();
        prune(&conn, now).unwrap();
        let hours = points(&conn, &row.id, "n", "hour", 0, now).unwrap();
        assert_eq!(hours, vec![(old.div_euclid(3600) * 3600, Some(3.0))]);
        assert_eq!(points(&conn, &row.id, "n", "raw", 0, now).unwrap(), vec![(now - 60, None)]);

        let t = crate::sources::state::Transition {
            scope_item: Some("b0".into()),
            from: Health::Ok,
            to: Health::Crit,
            kind: crate::sources::state::TransitionKind::WentCrit,
            incident_id: Some("i1".into()),
            reason: Some(Reason::Timeout),
            hard: true,
        };
        for i in 0..210 {
            insert_events(&conn, &row.id, i, std::slice::from_ref(&t), false).unwrap();
        }
        let ev = events(&conn, &row.id, 500).unwrap();
        assert_eq!(ev.len(), 200);
        assert_eq!(ev[0].at, 209);
        assert_eq!(ev[0].kind, "went_crit");
        audit(&conn, now, "agent:p1", "agent", "unmasked_read", Some(&row.id), None, &serde_json::json!({})).unwrap();
        assert!(audited_since(&conn, "agent:p1", "unmasked_read", &row.id, now - 1).unwrap());
        assert!(!audited_since(&conn, "agent:p1", "unmasked_read", &row.id, now + 1).unwrap());
    }
}
