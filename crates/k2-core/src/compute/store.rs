//! SQLite rows for compute (migration 0147). Every function takes the
//! connection so tests run on an isolated in-memory DB.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::proto::frames::{JobState, Offer, Plan};

// ── nodes ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRow {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    #[serde(skip)]
    pub public_key_pem: String,
    pub os: Option<String>,
    pub arch: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub state: String,
    pub enrolled_via: String,
    pub enrolled_by: Option<String>,
    pub enrolled_at: i64,
    #[serde(skip)]
    pub sas: Option<String>,
    pub confirmed_by: Option<String>,
    pub confirmed_at: Option<i64>,
    pub revoked_by: Option<String>,
    pub revoked_at: Option<i64>,
    pub controller_pause: Option<String>,
    pub controller_pause_by: Option<String>,
    pub controller_pause_at: Option<i64>,
    pub last_seen: Option<i64>,
    pub last_route: Option<String>,
    pub protocol: Option<i64>,
    pub node_version: Option<String>,
    pub routes: Vec<String>,
    #[serde(skip)]
    pub last_offer: Option<Offer>,
}

const NODE_COLS: &str = "id, name, fingerprint, public_key_pem, os, arch, labels_json, state, enrolled_via, \
    enrolled_by, enrolled_at, sas, confirmed_by, confirmed_at, revoked_by, revoked_at, controller_pause, \
    controller_pause_by, controller_pause_at, last_seen, last_route, protocol, node_version, routes_json, last_offer_json";

fn node_from_row(r: &Row<'_>) -> rusqlite::Result<NodeRow> {
    let labels: String = r.get(6)?;
    let routes: String = r.get(23)?;
    let offer: Option<String> = r.get(24)?;
    Ok(NodeRow {
        id: r.get(0)?,
        name: r.get(1)?,
        fingerprint: r.get(2)?,
        public_key_pem: r.get(3)?,
        os: r.get(4)?,
        arch: r.get(5)?,
        labels: serde_json::from_str(&labels).unwrap_or_default(),
        state: r.get(7)?,
        enrolled_via: r.get(8)?,
        enrolled_by: r.get(9)?,
        enrolled_at: r.get(10)?,
        sas: r.get(11)?,
        confirmed_by: r.get(12)?,
        confirmed_at: r.get(13)?,
        revoked_by: r.get(14)?,
        revoked_at: r.get(15)?,
        controller_pause: r.get(16)?,
        controller_pause_by: r.get(17)?,
        controller_pause_at: r.get(18)?,
        last_seen: r.get(19)?,
        last_route: r.get(20)?,
        protocol: r.get(21)?,
        node_version: r.get(22)?,
        routes: serde_json::from_str(&routes).unwrap_or_default(),
        last_offer: offer.and_then(|o| serde_json::from_str(&o).ok()),
    })
}

/// A new pending node (after a good enroll). Errors on a live name or
/// fingerprint clash (partial unique indexes).
#[allow(clippy::too_many_arguments)]
pub fn insert_pending_node(
    conn: &Connection,
    id: &str,
    name: &str,
    fingerprint: &str,
    public_key_pem: &str,
    labels: &BTreeMap<String, String>,
    enrolled_by: &str,
    sas: &str,
    at: i64,
) -> Result<NodeRow, String> {
    let os = labels.get("os").cloned();
    let arch = labels.get("arch").cloned();
    conn.execute(
        "INSERT INTO compute_nodes (id, name, fingerprint, public_key_pem, os, arch, labels_json, state, \
         enrolled_via, enrolled_by, enrolled_at, sas) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', 'code', ?8, ?9, ?10)",
        params![
            id,
            name,
            fingerprint,
            public_key_pem,
            os,
            arch,
            serde_json::to_string(labels).map_err(|e| e.to_string())?,
            enrolled_by,
            at,
            sas
        ],
    )
    .map_err(|e| e.to_string())?;
    node_by_id(conn, id).ok_or_else(|| "node vanished after insert".to_string())
}

pub fn node_by_id(conn: &Connection, id: &str) -> Option<NodeRow> {
    conn.query_row(&format!("SELECT {NODE_COLS} FROM compute_nodes WHERE id = ?1"), params![id], node_from_row)
        .optional()
        .ok()
        .flatten()
}

/// A live (not revoked) node by name.
pub fn live_node_by_name(conn: &Connection, name: &str) -> Option<NodeRow> {
    conn.query_row(
        &format!("SELECT {NODE_COLS} FROM compute_nodes WHERE name = ?1 AND state != 'revoked'"),
        params![name],
        node_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// A live (not revoked) node by fingerprint.
pub fn live_node_by_fp(conn: &Connection, fp: &str) -> Option<NodeRow> {
    conn.query_row(
        &format!("SELECT {NODE_COLS} FROM compute_nodes WHERE fingerprint = ?1 AND state != 'revoked'"),
        params![fp],
        node_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// The newest row for a fingerprint, revoked ones included (so a removed
/// node is told `revoked`, not `unknown_node`).
pub fn latest_node_by_fp(conn: &Connection, fp: &str) -> Option<NodeRow> {
    conn.query_row(
        &format!("SELECT {NODE_COLS} FROM compute_nodes WHERE fingerprint = ?1 ORDER BY enrolled_at DESC LIMIT 1"),
        params![fp],
        node_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Resolve a node token: id, or live name.
pub fn find_node(conn: &Connection, token: &str) -> Option<NodeRow> {
    let t = token.trim();
    live_node_by_name(conn, t).or_else(|| node_by_id(conn, t))
}

pub fn list_nodes(conn: &Connection, include_revoked: bool) -> Vec<NodeRow> {
    let sql = if include_revoked {
        format!("SELECT {NODE_COLS} FROM compute_nodes ORDER BY name")
    } else {
        format!("SELECT {NODE_COLS} FROM compute_nodes WHERE state != 'revoked' ORDER BY name")
    };
    let Ok(mut stmt) = conn.prepare(&sql) else { return Vec::new() };
    stmt.query_map([], node_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn confirm_node(conn: &Connection, id: &str, by: &str, at: i64) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE compute_nodes SET state = 'active', confirmed_by = ?2, confirmed_at = ?3, sas = NULL \
             WHERE id = ?1 AND state = 'pending'",
            params![id, by, at],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("node is not pending".to_string());
    }
    Ok(())
}

pub fn revoke_node(conn: &Connection, id: &str, by: &str, at: i64) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE compute_nodes SET state = 'revoked', revoked_by = ?2, revoked_at = ?3, sas = NULL \
             WHERE id = ?1 AND state != 'revoked'",
            params![id, by, at],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("node is already removed".to_string());
    }
    conn.execute("DELETE FROM compute_node_grants WHERE node_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn rename_node(conn: &Connection, id: &str, name: &str) -> Result<(), String> {
    conn.execute("UPDATE compute_nodes SET name = ?2 WHERE id = ?1", params![id, name])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

pub fn set_controller_pause(conn: &Connection, id: &str, pause: Option<&str>, by: &str, at: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE compute_nodes SET controller_pause = ?2, controller_pause_by = ?3, controller_pause_at = ?4 WHERE id = ?1",
        params![id, pause, by, at],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

pub fn set_routes(conn: &Connection, id: &str, routes: &[String]) -> Result<(), String> {
    conn.execute(
        "UPDATE compute_nodes SET routes_json = ?2 WHERE id = ?1",
        params![id, serde_json::to_string(routes).map_err(|e| e.to_string())?],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Record a live session's facts (route, protocol, version).
pub fn touch_seen(conn: &Connection, id: &str, route: &str, protocol: u32, version: &str, at: i64) {
    let _ = conn.execute(
        "UPDATE compute_nodes SET last_seen = ?2, last_route = ?3, protocol = ?4, node_version = ?5 WHERE id = ?1",
        params![id, at, route, protocol as i64, version],
    );
}

pub fn store_offer(conn: &Connection, id: &str, offer: &Offer, at: i64) {
    let labels = serde_json::to_string(&offer.labels).unwrap_or_else(|_| "{}".into());
    let _ = conn.execute(
        "UPDATE compute_nodes SET last_offer_json = ?2, last_seen = ?3, labels_json = ?4, \
         os = COALESCE(?5, os), arch = COALESCE(?6, arch) WHERE id = ?1",
        params![
            id,
            serde_json::to_string(offer).unwrap_or_default(),
            at,
            labels,
            offer.labels.get("os"),
            offer.labels.get("arch")
        ],
    );
}

// ── grants ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRow {
    pub node_id: String,
    pub workspace_id: String,
    pub scopes: String,
    pub max_job_secs: i64,
    pub max_disk_gb: i64,
    pub max_parallel: i64,
    pub max_queued: i64,
    pub warm_at: Option<String>,
    pub expires_at: Option<i64>,
    pub granted_by: String,
    pub granted_at: i64,
}

const GRANT_COLS: &str = "node_id, workspace_id, scopes, max_job_secs, max_disk_gb, max_parallel, max_queued, \
    warm_at, expires_at, granted_by, granted_at";

fn grant_from_row(r: &Row<'_>) -> rusqlite::Result<GrantRow> {
    Ok(GrantRow {
        node_id: r.get(0)?,
        workspace_id: r.get(1)?,
        scopes: r.get(2)?,
        max_job_secs: r.get(3)?,
        max_disk_gb: r.get(4)?,
        max_parallel: r.get(5)?,
        max_queued: r.get(6)?,
        warm_at: r.get(7)?,
        expires_at: r.get(8)?,
        granted_by: r.get(9)?,
        granted_at: r.get(10)?,
    })
}

/// Grant limits as set by the owner/admin (`None` = default).
#[derive(Debug, Clone, Default)]
pub struct GrantLimits {
    pub max_job_secs: Option<i64>,
    pub max_disk_gb: Option<i64>,
    pub max_parallel: Option<i64>,
    pub max_queued: Option<i64>,
}

pub fn set_grant(
    conn: &Connection,
    node_id: &str,
    workspace_id: &str,
    limits: &GrantLimits,
    by: &str,
    at: i64,
) -> Result<GrantRow, String> {
    conn.execute(
        "INSERT INTO compute_node_grants (node_id, workspace_id, max_job_secs, max_disk_gb, max_parallel, max_queued, \
         granted_by, granted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
         ON CONFLICT(node_id, workspace_id) DO UPDATE SET max_job_secs = excluded.max_job_secs, \
         max_disk_gb = excluded.max_disk_gb, max_parallel = excluded.max_parallel, max_queued = excluded.max_queued, \
         granted_by = excluded.granted_by, granted_at = excluded.granted_at",
        params![
            node_id,
            workspace_id,
            limits.max_job_secs.unwrap_or(super::DEFAULT_MAX_JOB_SECS),
            limits.max_disk_gb.unwrap_or(super::DEFAULT_MAX_DISK_GB),
            limits.max_parallel.unwrap_or(super::DEFAULT_MAX_PARALLEL),
            limits.max_queued.unwrap_or(super::DEFAULT_MAX_QUEUED),
            by,
            at
        ],
    )
    .map_err(|e| e.to_string())?;
    grant(conn, node_id, workspace_id).ok_or_else(|| "grant vanished after write".to_string())
}

pub fn revoke_grant(conn: &Connection, node_id: &str, workspace_id: &str) -> Result<bool, String> {
    conn.execute(
        "DELETE FROM compute_node_grants WHERE node_id = ?1 AND workspace_id = ?2",
        params![node_id, workspace_id],
    )
    .map(|n| n > 0)
    .map_err(|e| e.to_string())
}

pub fn grant(conn: &Connection, node_id: &str, workspace_id: &str) -> Option<GrantRow> {
    conn.query_row(
        &format!("SELECT {GRANT_COLS} FROM compute_node_grants WHERE node_id = ?1 AND workspace_id = ?2"),
        params![node_id, workspace_id],
        grant_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Grants filtered by node and/or workspace.
pub fn grants(conn: &Connection, node_id: Option<&str>, workspace_id: Option<&str>) -> Vec<GrantRow> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {GRANT_COLS} FROM compute_node_grants WHERE (?1 IS NULL OR node_id = ?1) \
         AND (?2 IS NULL OR workspace_id = ?2) ORDER BY node_id, workspace_id"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![node_id, workspace_id], grant_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

// ── jobs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub id: String,
    pub client_job_id: String,
    pub node_id: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub requested_by: String,
    pub argv: Vec<String>,
    pub env_names: Vec<String>,
    #[serde(skip)]
    pub request_sha256: String,
    #[serde(skip)]
    pub plan_json: String,
    pub src_commit: Option<String>,
    pub src_tree: Option<String>,
    pub dirty_sha256: Option<String>,
    pub state: String,
    pub reason: Option<String>,
    pub detail: Option<String>,
    pub attempt: i64,
    pub generation: i64,
    pub plan_digest: String,
    pub exclusive: bool,
    pub detach: bool,
    pub retry_interrupted: bool,
    pub exit_code: Option<i64>,
    pub signal: Option<i64>,
    pub created_at: i64,
    pub assigned_at: Option<i64>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub log_seq: i64,
    pub log_bytes: i64,
    pub summary_json: Option<String>,
    #[serde(skip)]
    pub receipt_json: Option<String>,
    pub receipt_ok: Option<bool>,
    pub notified_at: Option<i64>,
}

impl JobRow {
    pub fn plan(&self) -> Option<Plan> {
        serde_json::from_str(&self.plan_json).ok()
    }
    pub fn job_state(&self) -> Option<JobState> {
        JobState::parse(&self.state)
    }
    pub fn is_terminal(&self) -> bool {
        self.job_state().is_some_and(JobState::is_terminal)
    }
}

const JOB_COLS: &str = "id, client_job_id, node_id, workspace_id, session_id, requested_by, argv_json, env_names_json, \
    request_sha256, plan_json, src_commit, src_tree, dirty_sha256, state, reason, detail, attempt, generation, \
    plan_digest, exclusive, detach, retry_interrupted, exit_code, signal, created_at, assigned_at, started_at, \
    ended_at, log_seq, log_bytes, summary_json, receipt_json, receipt_ok, notified_at";

fn job_from_row(r: &Row<'_>) -> rusqlite::Result<JobRow> {
    let argv: String = r.get(6)?;
    let env: String = r.get(7)?;
    Ok(JobRow {
        id: r.get(0)?,
        client_job_id: r.get(1)?,
        node_id: r.get(2)?,
        workspace_id: r.get(3)?,
        session_id: r.get(4)?,
        requested_by: r.get(5)?,
        argv: serde_json::from_str(&argv).unwrap_or_default(),
        env_names: serde_json::from_str(&env).unwrap_or_default(),
        request_sha256: r.get(8)?,
        plan_json: r.get(9)?,
        src_commit: r.get(10)?,
        src_tree: r.get(11)?,
        dirty_sha256: r.get(12)?,
        state: r.get(13)?,
        reason: r.get(14)?,
        detail: r.get(15)?,
        attempt: r.get(16)?,
        generation: r.get(17)?,
        plan_digest: r.get(18)?,
        exclusive: r.get::<_, i64>(19)? != 0,
        detach: r.get::<_, i64>(20)? != 0,
        retry_interrupted: r.get::<_, i64>(21)? != 0,
        exit_code: r.get(22)?,
        signal: r.get(23)?,
        created_at: r.get(24)?,
        assigned_at: r.get(25)?,
        started_at: r.get(26)?,
        ended_at: r.get(27)?,
        log_seq: r.get(28)?,
        log_bytes: r.get(29)?,
        summary_json: r.get(30)?,
        receipt_json: r.get(31)?,
        receipt_ok: r.get::<_, Option<i64>>(32)?.map(|v| v != 0),
        notified_at: r.get(33)?,
    })
}

/// Everything a new job row needs.
#[derive(Debug, Clone)]
pub struct NewJob<'a> {
    pub plan: &'a Plan,
    pub client_job_id: &'a str,
    pub session_id: Option<&'a str>,
    pub request_sha256: &'a str,
    pub detach: bool,
    pub retry_interrupted: bool,
}

pub fn insert_job(conn: &Connection, j: &NewJob<'_>) -> Result<JobRow, String> {
    let p = j.plan;
    let digest = p.digest()?;
    let env_names: Vec<&String> = p.env.keys().collect();
    let src_commit = p.src.as_ref().map(|s| s.commit.clone());
    let dirty = p.src.as_ref().and_then(|s| s.dirty.as_ref().map(|d| d.sha256.clone()));
    conn.execute(
        "INSERT INTO compute_jobs (id, client_job_id, node_id, workspace_id, session_id, requested_by, argv_json, \
         env_names_json, request_sha256, plan_json, src_commit, dirty_sha256, state, plan_digest, exclusive, detach, \
         retry_interrupted, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'queued', ?13, ?14, \
         ?15, ?16, ?17)",
        params![
            p.job_id,
            j.client_job_id,
            p.node_id,
            p.workspace_id,
            j.session_id,
            p.requested_by,
            serde_json::to_string(&p.argv).map_err(|e| e.to_string())?,
            serde_json::to_string(&env_names).map_err(|e| e.to_string())?,
            j.request_sha256,
            serde_json::to_string(p).map_err(|e| e.to_string())?,
            src_commit,
            dirty,
            digest,
            p.exclusive as i64,
            j.detach as i64,
            j.retry_interrupted as i64,
            p.created_at
        ],
    )
    .map_err(|e| e.to_string())?;
    job_by_id(conn, &p.job_id).ok_or_else(|| "job vanished after insert".to_string())
}

pub fn job_by_id(conn: &Connection, id: &str) -> Option<JobRow> {
    conn.query_row(&format!("SELECT {JOB_COLS} FROM compute_jobs WHERE id = ?1"), params![id], job_from_row)
        .optional()
        .ok()
        .flatten()
}

/// A job by id or by an unambiguous id prefix (≥ 4 chars), as agents
/// type the short id the CLI prints.
pub fn find_job(conn: &Connection, token: &str) -> Option<JobRow> {
    let t = token.trim();
    if let Some(j) = job_by_id(conn, t) {
        return Some(j);
    }
    if t.len() < 4 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut stmt = conn
        .prepare(&format!("SELECT {JOB_COLS} FROM compute_jobs WHERE id LIKE ?1 || '%' LIMIT 2"))
        .ok()?;
    let rows: Vec<JobRow> = stmt.query_map(params![t], job_from_row).ok()?.filter_map(Result::ok).collect();
    if rows.len() == 1 {
        rows.into_iter().next()
    } else {
        None
    }
}

pub fn job_by_client_id(conn: &Connection, workspace_id: &str, client_job_id: &str) -> Option<JobRow> {
    conn.query_row(
        &format!("SELECT {JOB_COLS} FROM compute_jobs WHERE workspace_id = ?1 AND client_job_id = ?2"),
        params![workspace_id, client_job_id],
        job_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

/// Filter for [`list_jobs`].
#[derive(Debug, Clone, Default)]
pub struct JobFilter<'a> {
    pub node_id: Option<&'a str>,
    pub workspace_id: Option<&'a str>,
    pub state: Option<&'a str>,
    pub session_id: Option<&'a str>,
    pub limit: i64,
}

pub fn list_jobs(conn: &Connection, f: &JobFilter<'_>) -> Vec<JobRow> {
    let limit = if f.limit <= 0 { 50 } else { f.limit.min(500) };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {JOB_COLS} FROM compute_jobs WHERE (?1 IS NULL OR node_id = ?1) AND (?2 IS NULL OR workspace_id = ?2) \
         AND (?3 IS NULL OR state = ?3) AND (?4 IS NULL OR session_id = ?4) ORDER BY created_at DESC, id DESC LIMIT ?5"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![f.node_id, f.workspace_id, f.state, f.session_id, limit], job_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Jobs on `node_id` in any of `states`, oldest first.
pub fn jobs_in_states(conn: &Connection, node_id: &str, states: &[&str]) -> Vec<JobRow> {
    let list = states.iter().map(|s| format!("'{}'", s.replace('\'', ""))).collect::<Vec<_>>().join(",");
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {JOB_COLS} FROM compute_jobs WHERE node_id = ?1 AND state IN ({list}) ORDER BY created_at, id"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![node_id], job_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// Queued jobs for `workspace_id` on `node_id` (for the grant's max_queued).
pub fn queued_count(conn: &Connection, node_id: &str, workspace_id: &str) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM compute_jobs WHERE node_id = ?1 AND workspace_id = ?2 AND state = 'queued'",
        params![node_id, workspace_id],
        |r| r.get(0),
    )
    .unwrap_or(0)
}

/// Non-terminal states that count as "on the node".
pub const ACTIVE_STATES: &[&str] = &["assigned", "preparing", "running", "finishing"];

/// Set state (+ optional reason/detail); a terminal state stamps ended_at.
pub fn set_job_state(
    conn: &Connection,
    id: &str,
    state: JobState,
    reason: Option<&str>,
    detail: Option<&str>,
    at: i64,
) -> Result<(), String> {
    let terminal = state.is_terminal();
    conn.execute(
        "UPDATE compute_jobs SET state = ?2, reason = ?3, detail = ?4, \
         assigned_at = CASE WHEN ?2 = 'assigned' THEN ?5 ELSE assigned_at END, \
         started_at = CASE WHEN ?2 = 'running' AND started_at IS NULL THEN ?5 ELSE started_at END, \
         ended_at = CASE WHEN ?6 THEN COALESCE(ended_at, ?5) ELSE ended_at END WHERE id = ?1",
        params![id, state.as_str(), reason, detail, at, terminal],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

pub fn set_job_reason(conn: &Connection, id: &str, reason: Option<&str>, detail: Option<&str>) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET reason = ?2, detail = ?3 WHERE id = ?1",
        params![id, reason, detail],
    );
}

pub fn set_job_exit(conn: &Connection, id: &str, code: Option<i32>, signal: Option<i32>) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET exit_code = ?2, signal = ?3 WHERE id = ?1",
        params![id, code, signal],
    );
}

pub fn set_job_src(conn: &Connection, id: &str, commit: &str, tree: &str) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET src_commit = ?2, src_tree = ?3 WHERE id = ?1",
        params![id, commit, tree],
    );
}

pub fn set_job_log(conn: &Connection, id: &str, seq: u64, bytes: u64) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET log_seq = MAX(log_seq, ?2), log_bytes = MAX(log_bytes, ?3) WHERE id = ?1",
        params![id, seq as i64, bytes as i64],
    );
}

pub fn set_job_receipt(conn: &Connection, id: &str, receipt_json: &str, ok: bool) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET receipt_json = ?2, receipt_ok = ?3 WHERE id = ?1",
        params![id, receipt_json, ok as i64],
    );
}

pub fn mark_notified(conn: &Connection, id: &str, at: i64) {
    let _ = conn.execute("UPDATE compute_jobs SET notified_at = ?2 WHERE id = ?1", params![id, at]);
}

/// A new attempt: generation + 1, back to queued, results cleared.
pub fn requeue_new_attempt(conn: &Connection, id: &str, reason: &str) -> Result<JobRow, String> {
    conn.execute(
        "UPDATE compute_jobs SET state = 'queued', reason = ?2, detail = NULL, attempt = attempt + 1, \
         generation = generation + 1, exit_code = NULL, signal = NULL, assigned_at = NULL, started_at = NULL, \
         ended_at = NULL, log_seq = 0, log_bytes = 0, summary_json = NULL, receipt_json = NULL, receipt_ok = NULL, \
         notified_at = NULL WHERE id = ?1",
        params![id, reason],
    )
    .map_err(|e| e.to_string())?;
    job_by_id(conn, id).ok_or_else(|| "job vanished".to_string())
}

// ── usage and audit ──────────────────────────────────────────────────

pub fn add_usage(conn: &Connection, node_id: &str, day: &str, route: &str, bytes_in: u64, bytes_out: u64) {
    let _ = conn.execute(
        "INSERT INTO compute_usage (node_id, day, route, bytes_in, bytes_out) VALUES (?1, ?2, ?3, ?4, ?5) \
         ON CONFLICT(node_id, day, route) DO UPDATE SET bytes_in = bytes_in + excluded.bytes_in, \
         bytes_out = bytes_out + excluded.bytes_out",
        params![node_id, day, route, bytes_in as i64, bytes_out as i64],
    );
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageRow {
    pub node_id: String,
    pub day: String,
    pub route: String,
    pub bytes_in: i64,
    pub bytes_out: i64,
}

pub fn usage(conn: &Connection, since_day: &str) -> Vec<UsageRow> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT node_id, day, route, bytes_in, bytes_out FROM compute_usage WHERE day >= ?1 ORDER BY day DESC, node_id",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![since_day], |r| {
        Ok(UsageRow { node_id: r.get(0)?, day: r.get(1)?, route: r.get(2)?, bytes_in: r.get(3)?, bytes_out: r.get(4)? })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    pub id: i64,
    pub at: i64,
    pub actor: String,
    pub kind: String,
    pub node_id: Option<String>,
    pub workspace_id: Option<String>,
    pub job_id: Option<String>,
    pub detail: serde_json::Value,
}

/// One audit line (CN21). Never fails the caller.
pub fn record_event(
    conn: &Connection,
    actor: &str,
    kind: &str,
    node_id: Option<&str>,
    workspace_id: Option<&str>,
    job_id: Option<&str>,
    detail: serde_json::Value,
) {
    let _ = conn.execute(
        "INSERT INTO compute_events (at, actor, kind, node_id, workspace_id, job_id, detail_json) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![super::now(), actor, kind, node_id, workspace_id, job_id, detail.to_string()],
    );
}

pub fn events(conn: &Connection, limit: i64) -> Vec<EventRow> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT id, at, actor, kind, node_id, workspace_id, job_id, detail_json FROM compute_events \
         ORDER BY id DESC LIMIT ?1",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![limit.clamp(1, 1000)], |r| {
        let d: String = r.get(7)?;
        Ok(EventRow {
            id: r.get(0)?,
            at: r.get(1)?,
            actor: r.get(2)?,
            kind: r.get(3)?,
            node_id: r.get(4)?,
            workspace_id: r.get(5)?,
            job_id: r.get(6)?,
            detail: serde_json::from_str(&d).unwrap_or(serde_json::Value::Null),
        })
    })
    .map(|rows| rows.filter_map(Result::ok).collect())
    .unwrap_or_default()
}

// ── the workspace switch ─────────────────────────────────────────────

/// `projects.agents_can_use_compute` (fail closed on any error).
pub fn agents_can_use_compute(conn: &Connection, project_id: &str) -> bool {
    conn.query_row(
        "SELECT agents_can_use_compute FROM projects WHERE id = ?1",
        params![project_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|v| v == 1)
    .unwrap_or(false)
}

/// Dedicated writer for `/cli/agent-access/set` (CN20); never on the
/// generic `workspace/set` allowlist.
pub fn set_agents_can_use_compute(conn: &Connection, project_id: &str, on: bool) -> Result<(), String> {
    let n = conn
        .execute(
            "UPDATE projects SET agents_can_use_compute = ?1 WHERE id = ?2",
            params![on as i64, project_id],
        )
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Err(format!("Project not found: {project_id}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compute::proto::frames::{JobLimits, Src};

    fn conn() -> Connection {
        crate::db::isolated_test_connection()
    }

    fn labels() -> BTreeMap<String, String> {
        BTreeMap::from([("os".to_string(), "linux".to_string()), ("arch".to_string(), "x86_64".to_string())])
    }

    fn plan(id: &str, node: &str, ws: &str) -> Plan {
        Plan {
            job_id: id.into(),
            node_id: node.into(),
            workspace_id: ws.into(),
            workspace_label: "k2".into(),
            requested_by: "agent:k2".into(),
            argv: vec!["cargo".into(), "test".into()],
            env: BTreeMap::from([("RUST_LOG".into(), "info".into())]),
            cwd: None,
            limits: JobLimits { max_secs: 60, cpu_millis: None, mem_bytes: None, disk_bytes: 1, log_cap_bytes: 1 },
            src: Some(Src { project_key: "pk".into(), remote_url: None, commit: "c".repeat(40), dirty: None, slots: 1 }),
            exclusive: false,
            created_at: 10,
        }
    }

    #[test]
    fn node_lifecycle_pending_confirm_revoke_and_name_reuse() {
        let c = conn();
        let n = insert_pending_node(&c, "n1", "mini-1", "fp1", "pem", &labels(), "owner", "123456", 5).unwrap();
        assert_eq!(n.state, "pending");
        assert_eq!(n.os.as_deref(), Some("linux"));
        assert_eq!(n.sas.as_deref(), Some("123456"));
        // A second live node with the same name or key is refused.
        assert!(insert_pending_node(&c, "n2", "mini-1", "fp2", "pem", &labels(), "owner", "1", 5).is_err());
        assert!(insert_pending_node(&c, "n2", "mini-2", "fp1", "pem", &labels(), "owner", "1", 5).is_err());
        confirm_node(&c, "n1", "owner", 6).unwrap();
        assert!(confirm_node(&c, "n1", "owner", 6).is_err(), "confirm only works once");
        let n = node_by_id(&c, "n1").unwrap();
        assert_eq!(n.state, "active");
        assert_eq!(n.sas, None, "the SAS is dropped once confirmed");
        set_grant(&c, "n1", "ws1", &GrantLimits::default(), "owner", 7).unwrap();
        revoke_node(&c, "n1", "owner", 8).unwrap();
        assert!(grant(&c, "n1", "ws1").is_none(), "revoke drops grants");
        assert!(live_node_by_name(&c, "mini-1").is_none());
        assert_eq!(latest_node_by_fp(&c, "fp1").unwrap().state, "revoked");
        // The name and key are free again after a revoke.
        insert_pending_node(&c, "n3", "mini-1", "fp1", "pem", &labels(), "owner", "1", 9).unwrap();
        assert_eq!(find_node(&c, "mini-1").unwrap().id, "n3");
    }

    #[test]
    fn grants_upsert_with_defaults_and_overrides() {
        let c = conn();
        insert_pending_node(&c, "n1", "a", "fp", "pem", &labels(), "o", "1", 1).unwrap();
        let g = set_grant(&c, "n1", "ws", &GrantLimits::default(), "owner", 2).unwrap();
        assert_eq!((g.max_job_secs, g.max_parallel, g.max_queued, g.max_disk_gb), (7200, 1, 10, 60));
        let g = set_grant(&c, "n1", "ws", &GrantLimits { max_parallel: Some(3), ..Default::default() }, "admin", 3).unwrap();
        assert_eq!(g.max_parallel, 3);
        assert_eq!(g.granted_by, "admin");
        assert_eq!(grants(&c, Some("n1"), None).len(), 1);
        assert!(revoke_grant(&c, "n1", "ws").unwrap());
        assert!(!revoke_grant(&c, "n1", "ws").unwrap());
    }

    #[test]
    fn jobs_insert_find_state_and_requeue() {
        let c = conn();
        let p = plan("abcdef0123456789abcdef0123456789", "n1", "ws");
        let j = insert_job(
            &c,
            &NewJob { plan: &p, client_job_id: "client-0000000001", session_id: Some("s1"), request_sha256: "h", detach: true, retry_interrupted: true },
        )
        .unwrap();
        assert_eq!(j.state, "queued");
        assert_eq!(j.env_names, vec!["RUST_LOG".to_string()]);
        assert_eq!(j.plan().unwrap(), p);
        assert_eq!(find_job(&c, "abcdef01").unwrap().id, j.id);
        assert!(find_job(&c, "abc").is_none(), "too short a prefix");
        assert_eq!(job_by_client_id(&c, "ws", "client-0000000001").unwrap().id, j.id);
        assert!(insert_job(
            &c,
            &NewJob { plan: &plan("other", "n1", "ws"), client_job_id: "client-0000000001", session_id: None, request_sha256: "x", detach: false, retry_interrupted: false }
        )
        .is_err(), "client job id is unique per workspace");
        assert_eq!(queued_count(&c, "n1", "ws"), 1);
        set_job_state(&c, &j.id, JobState::Running, None, None, 20).unwrap();
        set_job_state(&c, &j.id, JobState::Done, None, None, 30).unwrap();
        set_job_exit(&c, &j.id, Some(1), None);
        let j2 = job_by_id(&c, &j.id).unwrap();
        assert_eq!((j2.started_at, j2.ended_at, j2.exit_code), (Some(20), Some(30), Some(1)));
        assert!(j2.is_terminal());
        let r = requeue_new_attempt(&c, &j.id, "retry").unwrap();
        assert_eq!((r.state.as_str(), r.attempt, r.generation, r.exit_code, r.ended_at), ("queued", 2, 2, None, None));
        assert_eq!(jobs_in_states(&c, "n1", &["queued"]).len(), 1);
    }

    #[test]
    fn usage_accumulates_and_events_record() {
        let c = conn();
        add_usage(&c, "n1", "2026-10-08", "relay", 10, 20);
        add_usage(&c, "n1", "2026-10-08", "relay", 1, 2);
        add_usage(&c, "n1", "2026-10-08", "lan", 5, 5);
        let u = usage(&c, "2026-10-01");
        let relay = u.iter().find(|r| r.route == "relay").unwrap();
        assert_eq!((relay.bytes_in, relay.bytes_out), (11, 22));
        record_event(&c, "owner", "enroll", Some("n1"), None, None, serde_json::json!({"name": "a"}));
        let e = events(&c, 10);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].detail["name"], "a");
    }

    #[test]
    fn workspace_switch_defaults_off() {
        let c = conn();
        c.execute(
            "INSERT INTO projects (id, path, name) VALUES ('p1', '/tmp/p1', 'p1')",
            [],
        )
        .unwrap();
        assert!(!agents_can_use_compute(&c, "p1"));
        set_agents_can_use_compute(&c, "p1", true).unwrap();
        assert!(agents_can_use_compute(&c, "p1"));
        assert!(set_agents_can_use_compute(&c, "nope", true).is_err());
        assert!(!agents_can_use_compute(&c, "nope"));
    }
}
