//! SQLite rows for compute (migration 0147). Every function takes the
//! connection so tests run on an isolated in-memory DB.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

use super::proto::frames::{JobState, Offer, Plan};
use super::proto::share::{QueuePos, ShareView};

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
    /// 0214 (node sharing, 0.46.3): `owner` (this server enrolled the
    /// node first) or `tenant` (another server owns it). Never serialized
    /// here: the routes add the share fields only for shared nodes, so a
    /// solo node's JSON is unchanged.
    #[serde(skip)]
    pub role: String,
    /// Two or more servers active on the node (its last `Offer.share`).
    #[serde(skip)]
    pub shared: bool,
    /// The owner server's label ("shared by …"), for tenants.
    #[serde(skip)]
    pub owner_label: Option<String>,
    /// The last `ShareView` and when it arrived.
    #[serde(skip)]
    pub share: Option<ShareView>,
    #[serde(skip)]
    pub share_at: Option<i64>,
}

impl NodeRow {
    /// Another server owns this node (V52: no admin, updates or health).
    pub fn is_tenant(&self) -> bool {
        self.role == "tenant"
    }

    /// This server owns the node for secret-env purposes (`compute:<ws>/*`
    /// covers it, Q2): never a tenant (sticky, from enrolment) and never
    /// given an owner label (only a node another server owns has one).
    /// Nothing the node's offer says can make this true (review A-H1).
    pub fn owned_for_secrets(&self) -> bool {
        !self.is_tenant() && self.owner_label.is_none()
    }

    /// The node says this server is its owner now while this server still
    /// has it as a tenant (an owner change on the node, prd-macos-tenant-
    /// compute-v1 §6.3): its last offer's share view says `owner`, or (an
    /// offer with no view) it lists a feature only the owner's session
    /// gets. Only a claim: the node is never taken at its word. The row
    /// turns owned only through [`mark_node_owned`], after the person here
    /// confirms (`POST /cli/compute/nodes/take-owner`).
    pub fn owner_claim(&self) -> bool {
        if !self.is_tenant() {
            return false;
        }
        match self.last_offer.as_ref() {
            Some(o) => super::share::offer_role(o) == Some(super::proto::share::Role::Owner),
            None => false,
        }
    }
}

const NODE_COLS: &str = "id, name, fingerprint, public_key_pem, os, arch, labels_json, state, enrolled_via, \
    enrolled_by, enrolled_at, sas, confirmed_by, confirmed_at, revoked_by, revoked_at, controller_pause, \
    controller_pause_by, controller_pause_at, last_seen, last_route, protocol, node_version, routes_json, last_offer_json, \
    role, shared, owner_label, share_json, share_at";

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
        role: r.get(25)?,
        shared: r.get::<_, i64>(26)? != 0,
        owner_label: r.get(27)?,
        share: r.get::<_, Option<String>>(28)?.and_then(|s| serde_json::from_str(&s).ok()),
        share_at: r.get(29)?,
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
    insert_pending_node_via(conn, id, name, fingerprint, public_key_pem, labels, enrolled_by, sas, at, "code")
}

/// [`insert_pending_node`] with the enroll path (`code` | `ticket`).
#[allow(clippy::too_many_arguments)]
pub fn insert_pending_node_via(
    conn: &Connection,
    id: &str,
    name: &str,
    fingerprint: &str,
    public_key_pem: &str,
    labels: &BTreeMap<String, String>,
    enrolled_by: &str,
    sas: &str,
    at: i64,
    via: &str,
) -> Result<NodeRow, String> {
    if via != "code" && via != "ticket" {
        return Err(format!("enrolled_via {via:?}"));
    }
    let os = labels.get("os").cloned();
    let arch = labels.get("arch").cloned();
    conn.execute(
        "INSERT INTO compute_nodes (id, name, fingerprint, public_key_pem, os, arch, labels_json, state, \
         enrolled_via, enrolled_by, enrolled_at, sas) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?11, ?8, ?9, ?10)",
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
            sas,
            via
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

/// Mirror what the node's offer says about this server's share (0214).
/// `share = None` (a solo node, or one older than 0.46.3) clears
/// `shared` and the view but keeps the role: a tenant stays a tenant.
pub fn store_share(conn: &Connection, id: &str, share: Option<&ShareView>, at: i64) {
    let _ = match share {
        // The role is sticky one way (security review A-H1): an offer may
        // turn an owned node into a tenant one, never a tenant back into
        // owned. `tenant` comes from the enrolment (`set_tenant_of`,
        // `EnrollRequest.shared_by`) or an earlier offer, and a node's own
        // offer can't lift it.
        Some(v) => conn.execute(
            "UPDATE compute_nodes SET role = CASE WHEN role = 'tenant' THEN 'tenant' ELSE ?2 END, shared = ?3, \
             owner_label = COALESCE(?4, owner_label), share_json = ?5, share_at = ?6 WHERE id = ?1",
            params![id, v.role.as_str(), v.shared as i64, v.owner_label, serde_json::to_string(v).unwrap_or_default(), at],
        ),
        None => conn.execute(
            "UPDATE compute_nodes SET shared = 0, share_json = NULL, share_at = ?2 WHERE id = ?1 AND share_json IS NOT NULL",
            params![id, at],
        ),
    };
}

/// The one way a node becomes this server's own after enrolment (for the
/// owner-change flow): call it ONLY after a person approved the change and
/// the node confirmed it, never from an offer. It clears the tenant role
/// and the owner label, so [`NodeRow::owned_for_secrets`] becomes true and
/// `compute:<ws>/*` grants cover the node from then on.
pub fn mark_node_owned(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute("UPDATE compute_nodes SET role = 'owner', owner_label = NULL WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

/// A node another server's `servers.add` enrolled with this server
/// (`EnrollRequest.shared_by`): this server is a tenant there.
pub fn set_tenant_of(conn: &Connection, id: &str, owner_label: &str) -> Result<(), String> {
    conn.execute("UPDATE compute_nodes SET role = 'tenant', owner_label = ?2 WHERE id = ?1", params![id, owner_label])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

// ── a node's servers (owner side, 0216) ──────────────────────────────

/// The mirrored server list of a node this server owns, by slot.
pub fn node_servers(conn: &Connection, node_id: &str) -> Vec<super::share::ServerRow> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT slot, label, fp, state, trust, share_json, usage_json, expires_at, added_at, added_by FROM compute_node_servers \
         WHERE node_id = ?1 ORDER BY slot",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![node_id], |r| {
        let limits: String = r.get(5)?;
        let usage: Option<String> = r.get(6)?;
        Ok((
            super::share::ServerRow {
                slot: r.get::<_, i64>(0)? as u8,
                label: r.get(1)?,
                fp: r.get(2)?,
                state: r.get(3)?,
                trust: r.get(4)?,
                limits: super::proto::share::ShareLimits::new_server(super::proto::share::Trust::Team, false),
                usage: usage.and_then(|u| serde_json::from_str(&u).ok()),
                expires_at: r.get(7)?,
                added_at: r.get(8)?,
                added_by: r.get(9)?,
            },
            limits,
        ))
    })
    .map(|rows| {
        rows.filter_map(Result::ok)
            // A row whose limits don't parse is dropped, never shown with
            // made-up limits (the next refresh rewrites it).
            .filter_map(|(mut row, limits)| serde_json::from_str(&limits).ok().map(|l| {
                row.limits = l;
                row
            }))
            .collect()
    })
    .unwrap_or_default()
}

/// When the node's server list was last mirrored.
pub fn node_servers_seen_at(conn: &Connection, node_id: &str) -> Option<i64> {
    conn.query_row("SELECT MAX(seen_at) FROM compute_node_servers WHERE node_id = ?1", params![node_id], |r| r.get(0))
        .ok()
        .flatten()
}

/// Replace a node's mirrored server list with what `servers.list` said.
pub fn replace_node_servers(conn: &Connection, node_id: &str, rows: &[super::share::ServerRow], at: i64) -> Result<(), String> {
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM compute_node_servers WHERE node_id = ?1", params![node_id]).map_err(|e| e.to_string())?;
    for r in rows {
        tx.execute(
            "INSERT INTO compute_node_servers (node_id, slot, label, fp, state, trust, share_json, usage_json, expires_at, added_at, \
             added_by, seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                node_id,
                r.slot as i64,
                r.label,
                r.fp,
                r.state,
                r.trust,
                serde_json::to_string(&r.limits).map_err(|e| e.to_string())?,
                r.usage.as_ref().map(|u| u.to_string()),
                r.expires_at,
                r.added_at,
                r.added_by,
                at
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
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
    /// 0230: this workspace may send CI jobs to this node (owner or Admin
    /// only; prd-compute-job-tiers-v1 §5.1, T1).
    pub allow_ci: bool,
    /// 0241 (prd-compute-job-tiers-v2 L3): at most this many running jobs
    /// per submitter of this workspace on this node; 0 = off (the default).
    /// CI never counts.
    pub submitter_max_running: i64,
}

const GRANT_COLS: &str = "node_id, workspace_id, scopes, max_job_secs, max_disk_gb, max_parallel, max_queued, \
    warm_at, expires_at, granted_by, granted_at, allow_ci, submitter_max_running";

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
        allow_ci: r.get::<_, i64>(11)? != 0,
        submitter_max_running: r.get(12)?,
    })
}

/// Grant limits as set by the owner/admin (`None` = default).
#[derive(Debug, Clone, Default)]
pub struct GrantLimits {
    pub max_job_secs: Option<i64>,
    pub max_disk_gb: Option<i64>,
    pub max_parallel: Option<i64>,
    pub max_queued: Option<i64>,
    /// `None` keeps what the grant has (a new grant: off). Re-granting
    /// limits never silently sets or drops CI (T1).
    pub allow_ci: Option<bool>,
    /// `None` keeps what the grant has (a new grant: 0, off), like
    /// `allow_ci`: re-granting limits never clears it (T30).
    pub submitter_max_running: Option<i64>,
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
         granted_by, granted_at, allow_ci, submitter_max_running) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, COALESCE(?9, 0), COALESCE(?10, 0)) \
         ON CONFLICT(node_id, workspace_id) DO UPDATE SET max_job_secs = excluded.max_job_secs, \
         max_disk_gb = excluded.max_disk_gb, max_parallel = excluded.max_parallel, max_queued = excluded.max_queued, \
         granted_by = excluded.granted_by, granted_at = excluded.granted_at, \
         allow_ci = COALESCE(?9, compute_node_grants.allow_ci), \
         submitter_max_running = COALESCE(?10, compute_node_grants.submitter_max_running)",
        params![
            node_id,
            workspace_id,
            limits.max_job_secs.unwrap_or(super::DEFAULT_MAX_JOB_SECS),
            limits.max_disk_gb.unwrap_or(super::DEFAULT_MAX_DISK_GB),
            limits.max_parallel.unwrap_or(super::DEFAULT_MAX_PARALLEL),
            limits.max_queued.unwrap_or(super::DEFAULT_MAX_QUEUED),
            by,
            at,
            limits.allow_ci.map(i64::from),
            limits.submitter_max_running
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

/// The key under which [`JobCtl`] rides in `compute_jobs.plan_json`
/// (beside the plan's own fields; never sent to a node, never in the
/// plan digest: `Plan` ignores it when it reads the row back).
pub const CTL_KEY: &str = "k2ctl";

/// Controller-only options and bookkeeping for one job (no migration:
/// it lives in `plan_json`). Never-stuck placement, retries, shards.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCtl {
    /// `--any`: what any node must match, so the job may move.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub any: Option<AnyFilter>,
    /// Submitted by the owner or an admin (may use ungranted nodes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub admin: bool,
    /// `--timeout` as asked (None = the grant's max), re-applied when the
    /// job moves to a node with another grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<i64>,
    /// Give up when not started this long after `queued_at` (None = wait).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue_timeout_secs: Option<i64>,
    /// When the job last entered the queue (submit, retry).
    #[serde(default)]
    pub queued_at: i64,
    /// Infrastructure retries allowed and used.
    #[serde(default)]
    pub retries: u32,
    #[serde(default)]
    pub retries_used: u32,
    /// Retry backoff: not placed before this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<i64>,
    /// Earlier attempts that ended in an infrastructure failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_attempts: Vec<AttemptNote>,
    /// `--stall-timeout` as asked: None = the default, 0 = off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_secs: Option<i64>,
    /// A shard of a split run: its parent and place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shard: Option<ShardRef>,
    /// A split run's parent: the child job ids, shard 1 first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shards: Vec<String>,
    /// Whoever asked for the job also cancelled it: ending `cancelled` is
    /// no news to them, so no `[compute]` completion message.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cancelled_by_requester: bool,
    /// Compute secret-env (SE4): the vault secrets this job gets, pinned at
    /// submit by id. Names and ids only, never a value; controller-only
    /// (never sent: `Plan.secret_env` carries the env names to the node).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretPin>,
    /// `--supersede`: a newer job with the same key replaced this queued
    /// one (its id). A node-queued job's cancel then ends `replaced`, not
    /// `cancelled` (prd-compute-job-tiers-v1 §3.4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaced_by: Option<String>,
    /// `--repo` (0.46.6): the nested repo the job's code comes from, its
    /// main checkout relative to the workspace folder (`/`-joined; checked
    /// again by `sync::nested_repo` whenever a bundle is built from it).
    /// `None` = the workspace's own repo. Controller-only: the node only
    /// ever sees the commit and the project key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// `--priority --why "<reason>"` (prd-compute-job-tiers-v2, Rosson
    /// 10-28): one line, shown in the queue and `k2 compute status`.
    /// Controller-only (never sent to a node).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
    /// No `--mem` was given: the plan reserves the default (and the node
    /// stops the job above it). Shown in the `mem_cap` hint.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mem_defaulted: bool,
    /// Where the job was placed and why (prd-compute-auto-placement-v1 §7,
    /// CP-A10): written at submit, rewritten by a move. Controller-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<Placement>,
    /// The run's effective settings and where each came from (`flag`,
    /// `preset`, `defaults`): `.k2/compute.toml` and `--preset`
    /// (prd-compute-easy-mode-0467 CE-3), plus the CPU hints K2 added
    /// (CE-15). Env names only, never a value. Controller-only; `get
    /// --json` shows it as `result.effective`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<serde_json::Value>,
    /// `--tag NAME` (CE-9, D2): the job's own label (≤ 64 of
    /// `A-Za-z0-9._:/-`). `jobs --tag` filters on `$.k2ctl.tag`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// CE-15: the env names K2 set to the job's CPUs (the caller set none
    /// of them), so a move recomputes them for the new node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cpu_hints: Vec<String>,
    /// Whose clock the job's start and end times are (CE-5): `node` (the
    /// node's own time; its clock agrees with this server's) or `server`
    /// (the node's clock is off, or couldn't be checked: this server's
    /// arrival time). Set at the first observation that carries a time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<String>,
    /// CE-5: this server heard of the job's end this many seconds after the
    /// node's own end time (only when more than [`REPORTED_LATE_SECS`]),
    /// and when it heard (`heardAt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reported_late_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heard_at: Option<i64>,
    /// CE-10: the stall warning went out for this attempt (its generation),
    /// so it is sent once per attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_warned_gen: Option<i64>,
}

/// CE-5: an end heard more than this long after the node's own time is
/// "reported late"; a node clock off by more than this isn't trusted.
pub const REPORTED_LATE_SECS: i64 = 120;

/// Longest `--tag`.
pub const TAG_MAX: usize = 64;

/// A job tag (`--tag`, CE-9): 1–64 of `A-Z a-z 0-9 . _ : / -`.
pub fn valid_tag(t: &str) -> bool {
    !t.is_empty() && t.len() <= TAG_MAX && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '/' | '-'))
}

/// Where a job was placed and why (prd-compute-auto-placement-v1 §7).
/// Built only from node names this workspace is granted, node-wide numbers
/// `k2 compute nodes` already shows, and this workspace's own slots and
/// jobs: never another workspace's or server's job, argv or handle (S2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    /// `auto` (no node named, or `--any`) or `pinned` (a node named).
    pub mode: String,
    /// The ranking that picked it: `v2` or `v1` (`computePlacement`).
    pub policy: String,
    /// The node's name.
    pub node: String,
    /// `starts_now_warm` | `starts_now_idle` | `starts_now_least_loaded` |
    /// `starts_now_spread` | `queued_quoted_eta` | `queued_least_busy` |
    /// `queued_offline_only` | `moved_<why>` | `pinned`.
    pub code: String,
    /// One line: why this node.
    pub why: String,
    /// "6 nodes fit, 2 skipped (1 offline, 1 too small for --mem 24G)".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fits: String,
    /// Starts now (as far as the last offer says); `None` when pinned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub starts_now: Option<bool>,
    /// The node's quoted wait, when it queued on a shared node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eta_secs: Option<u64>,
    /// Granted nodes that could take it.
    #[serde(default)]
    pub considered: u32,
    /// Granted nodes left out, by reason code.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub skipped: BTreeMap<String, u32>,
    pub at: i64,
}

/// One secret a compute job gets (SE4): what was asked, the vault id it
/// resolved to at submit, and the env name the job sees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretPin {
    /// As the submitter wrote it (`NAME` or a `k2v://` reference).
    pub name: String,
    pub secret_id: String,
    pub env: String,
}

/// Which shard a child job is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShardRef {
    pub parent: String,
    /// 1-based.
    pub index: u32,
    pub of: u32,
}

/// What an `--any` job asks of a node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnyFilter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arch: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
}

/// One attempt that ended in an infrastructure failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttemptNote {
    pub attempt: i64,
    pub node_id: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub at: i64,
}

/// `plan_json` text: the plan's own fields plus `k2ctl` (when not empty).
pub fn plan_json_with(plan: &Plan, ctl: &JobCtl) -> Result<String, String> {
    let mut v = serde_json::to_value(plan).map_err(|e| e.to_string())?;
    if *ctl != JobCtl::default() {
        if let Some(m) = v.as_object_mut() {
            m.insert(CTL_KEY.into(), serde_json::to_value(ctl).map_err(|e| e.to_string())?);
        }
    }
    serde_json::to_string(&v).map_err(|e| e.to_string())
}

fn ctl_of(plan_json: &str) -> JobCtl {
    serde_json::from_str::<serde_json::Value>(plan_json)
        .ok()
        .and_then(|v| v.get(CTL_KEY).cloned())
        .and_then(|c| serde_json::from_value(c).ok())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
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
    /// Controller-only options (from `plan_json`).
    pub ctl: JobCtl,
    /// 0215 (node sharing, 0.46.3). Never serialized here: the routes show
    /// a node-queued job as queued with the node's position, and a solo
    /// node's job JSON stays as it was.
    #[serde(skip)]
    pub class: String,
    #[serde(skip)]
    pub est_secs: Option<i64>,
    /// The node holds the job in its queue: the row stays `assigned`
    /// (reason `node_queued`), every view shows it as queued.
    #[serde(skip)]
    pub node_queued: bool,
    #[serde(skip)]
    pub node_pos: Option<i64>,
    #[serde(skip)]
    pub node_eta_secs: Option<i64>,
    #[serde(skip)]
    pub node_eta_p90_secs: Option<i64>,
    #[serde(skip)]
    pub node_blocked_by: Option<String>,
    #[serde(skip)]
    pub node_seen_at: Option<i64>,
    /// The `--any` mover's withdraws (V71).
    #[serde(skip)]
    pub withdraws: i64,
    #[serde(skip)]
    pub withdrawn_at: Option<i64>,
    /// 0230 (prd-compute-job-tiers-v1): `ci` | `priority` | `normal`, the
    /// tier the queue orders it by.
    pub tier: String,
    /// What the caller asked when the server lowered it (a guest share
    /// where Priority doesn't count).
    pub tier_asked: Option<String>,
    /// Who sent it (display and the Priority note; not security).
    pub submitter: Option<String>,
    /// `--supersede KEY`.
    pub supersede_key: Option<String>,
    /// How many jobs wait in the node's queue ("N of M"; 0240,
    /// prd-compute-job-tiers-v2 L6). Absent from a node older than 0.46.6.
    #[serde(skip)]
    pub node_of: Option<i64>,
}

impl JobRow {
    /// The tier the queue orders it by (an unknown word is Normal).
    pub fn tier(&self) -> super::proto::tiers::Tier {
        super::proto::tiers::Tier::parse(&self.tier).unwrap_or_default()
    }
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
    ended_at, log_seq, log_bytes, summary_json, receipt_json, receipt_ok, notified_at, class, est_secs, node_queued, \
    node_pos, node_eta_secs, node_eta_p90_secs, node_blocked_by, node_seen_at, withdraws, withdrawn_at, tier, tier_asked, \
    submitter, supersede_key, node_of";

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
        ctl: ctl_of(&r.get::<_, String>(9)?),
        class: r.get(34)?,
        est_secs: r.get(35)?,
        node_queued: r.get::<_, i64>(36)? != 0,
        node_pos: r.get(37)?,
        node_eta_secs: r.get(38)?,
        node_eta_p90_secs: r.get(39)?,
        node_blocked_by: r.get(40)?,
        node_seen_at: r.get(41)?,
        withdraws: r.get(42)?,
        withdrawn_at: r.get(43)?,
        tier: r.get(44)?,
        tier_asked: r.get(45)?,
        submitter: r.get(46)?,
        supersede_key: r.get(47)?, node_of: r.get(48)?,
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
    pub ctl: JobCtl,
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
            plan_json_with(p, &j.ctl)?,
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
    /// `ci` | `priority` | `normal` (0230).
    pub tier: Option<&'a str>,
    /// `--tag` (CE-9): in SQL, so `limit` counts only matching jobs.
    pub tag: Option<&'a str>,
}

pub fn list_jobs(conn: &Connection, f: &JobFilter<'_>) -> Vec<JobRow> {
    let limit = if f.limit <= 0 { 50 } else { f.limit.min(500) };
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {JOB_COLS} FROM compute_jobs WHERE (?1 IS NULL OR node_id = ?1) AND (?2 IS NULL OR workspace_id = ?2) \
         AND (?3 IS NULL OR state = ?3 OR (?3 = 'queued' AND state = 'assigned' AND node_queued = 1)) AND (?4 IS NULL OR session_id = ?4) \
         AND (?6 IS NULL OR tier = ?6) AND (?7 IS NULL OR json_extract(plan_json, '$.k2ctl.tag') = ?7) \
         ORDER BY created_at DESC, id DESC LIMIT ?5"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![f.node_id, f.workspace_id, f.state, f.session_id, limit, f.tier, f.tag], job_from_row)
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
        "SELECT COUNT(*) FROM compute_jobs WHERE node_id = ?1 AND workspace_id = ?2 AND (state = 'queued' OR (state = 'assigned' AND node_queued = 1))",
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

/// Compare-and-set: like [`set_job_state`], but only while the row is still
/// in state `from`. `Ok(true)` when it moved. The controller's placement
/// (`kick`) and the cancel route both write a queued row; neither may
/// overwrite the other's decision.
pub fn set_job_state_if(
    conn: &Connection,
    id: &str,
    from: &str,
    state: JobState,
    reason: Option<&str>,
    detail: Option<&str>,
    at: i64,
) -> Result<bool, String> {
    let terminal = state.is_terminal();
    conn.execute(
        "UPDATE compute_jobs SET state = ?2, reason = ?3, detail = ?4, \
         assigned_at = CASE WHEN ?2 = 'assigned' THEN ?5 ELSE assigned_at END, \
         started_at = CASE WHEN ?2 = 'running' AND started_at IS NULL THEN ?5 ELSE started_at END, \
         ended_at = CASE WHEN ?6 THEN COALESCE(ended_at, ?5) ELSE ended_at END WHERE id = ?1 AND state = ?7",
        params![id, state.as_str(), reason, detail, at, terminal, from],
    )
    .map_err(|e| e.to_string())
    .map(|n| n == 1)
}

/// Cancel a job only while it is still queued (never an assigned or
/// running one: those need a `Cancel` frame to the node).
pub fn cancel_if_queued(conn: &Connection, id: &str, at: i64) -> Result<bool, String> {
    set_job_state_if(conn, id, "queued", JobState::Cancelled, Some("cancelled"), None, at)
}

/// Assign a job only while it is still queued (a cancel that landed first wins).
pub fn assign_if_queued(conn: &Connection, id: &str, at: i64) -> Result<bool, String> {
    set_job_state_if(conn, id, "queued", JobState::Assigned, None, None, at)
}

pub fn set_job_reason(conn: &Connection, id: &str, reason: Option<&str>, detail: Option<&str>) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET reason = ?2, detail = ?3 WHERE id = ?1",
        params![id, reason, detail],
    );
}

/// CE-5: put the node's own start and/or end time on a job (already
/// clamped by the caller: ended >= started >= assigned). `None` leaves a
/// time as it is.
pub fn set_job_times(conn: &Connection, id: &str, started_at: Option<i64>, ended_at: Option<i64>) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET started_at = COALESCE(?2, started_at), ended_at = COALESCE(?3, ended_at) WHERE id = ?1",
        params![id, started_at, ended_at],
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

/// Store the node-reported outputs (artifacts + test summary) for the
/// attempt `generation` only (a stale attempt's outputs never land).
pub fn set_job_outputs(conn: &Connection, id: &str, generation: i64, outputs_json: &str) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET summary_json = ?3 WHERE id = ?1 AND generation = ?2",
        params![id, generation, outputs_json],
    );
}

pub fn set_job_receipt(conn: &Connection, id: &str, receipt_json: &str, ok: bool) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET receipt_json = ?2, receipt_ok = ?3 WHERE id = ?1",
        params![id, receipt_json, ok as i64],
    );
}

/// Rewrite a job's controller options (the plan itself is unchanged).
pub fn set_job_ctl(conn: &Connection, id: &str, ctl: &JobCtl) -> Result<(), String> {
    let Some(j) = job_by_id(conn, id) else { return Err("job not found".into()) };
    let plan = j.plan().ok_or_else(|| "stored plan unreadable".to_string())?;
    conn.execute("UPDATE compute_jobs SET plan_json = ?2 WHERE id = ?1", params![id, plan_json_with(&plan, ctl)?])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

/// Move a QUEUED job to another node with its rebuilt plan (new digest).
/// Returns false when the job wasn't queued any more (it raced a placement).
pub fn move_queued_job(conn: &Connection, id: &str, plan: &Plan, ctl: &JobCtl, reason: &str) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE compute_jobs SET node_id = ?2, plan_json = ?3, plan_digest = ?4, reason = ?5, detail = NULL \
             WHERE id = ?1 AND state = 'queued'",
            params![id, plan.node_id, plan_json_with(plan, ctl)?, plan.digest()?, reason],
        )
        .map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// Rewrite a still-queued job's plan in place, same node: its JSON and
/// digest (job tiers T11: the tier follows the node's features when the
/// job is sent). `Ok(false)` when it isn't queued any more.
pub fn rewrite_queued_plan(conn: &Connection, id: &str, plan: &Plan, ctl: &JobCtl) -> Result<bool, String> {
    let n = conn
        .execute(
            "UPDATE compute_jobs SET plan_json = ?2, plan_digest = ?3 WHERE id = ?1 AND state = 'queued'",
            params![id, plan_json_with(plan, ctl)?, plan.digest()?],
        )
        .map_err(|e| e.to_string())?;
    Ok(n == 1)
}

/// A split run's parent is `running` (no node) until its shards end.
pub fn set_parent_running(conn: &Connection, id: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE compute_jobs SET state = 'running', reason = NULL, detail = NULL, exit_code = NULL, signal = NULL, \
         ended_at = NULL, summary_json = NULL, notified_at = NULL WHERE id = ?1",
        params![id],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Write a split run's combined end (once every shard has ended).
pub fn finish_parent(conn: &Connection, id: &str, a: &super::shards::Aggregate) -> Result<(), String> {
    let summary = a
        .tests
        .as_ref()
        .map(|t| serde_json::to_string(&super::proto::results::Outputs { tests: Some(t.clone()), ..Default::default() }))
        .transpose()
        .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE compute_jobs SET state = ?2, reason = ?3, detail = ?4, exit_code = ?5, signal = ?6, started_at = ?7, \
         ended_at = COALESCE(?8, ended_at), summary_json = ?9 WHERE id = ?1",
        params![id, a.state, a.reason, a.detail, a.exit_code, a.signal, a.started_at, a.ended_at, summary],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Wall times (seconds) of recent finished runs of exactly `argv` in a
/// workspace, newest first; on `node_id` only when given.
pub fn recent_durations(conn: &Connection, workspace_id: &str, argv: &[String], node_id: Option<&str>, limit: i64) -> Vec<i64> {
    let Ok(argv_json) = serde_json::to_string(argv) else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT ended_at - started_at FROM compute_jobs WHERE workspace_id = ?1 AND argv_json = ?2 AND state = 'done' \
         AND started_at IS NOT NULL AND ended_at IS NOT NULL AND (?3 IS NULL OR node_id = ?3) ORDER BY ended_at DESC LIMIT ?4",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![workspace_id, argv_json, node_id, limit.clamp(1, 200)], |r| r.get::<_, i64>(0))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// `(median, p90)` of durations (nearest rank).
pub fn median_p90(samples: &[i64]) -> Option<(i64, i64)> {
    if samples.is_empty() {
        return None;
    }
    let mut v = samples.to_vec();
    v.sort_unstable();
    let rank = |q: f64| v[(((q * v.len() as f64).ceil() as usize).max(1) - 1).min(v.len() - 1)];
    Some((rank(0.5), rank(0.9)))
}

/// Every queued job (all nodes), oldest first.
pub fn queued_jobs(conn: &Connection) -> Vec<JobRow> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {JOB_COLS} FROM compute_jobs WHERE state = 'queued' ORDER BY created_at, id")) else {
        return Vec::new();
    };
    stmt.query_map([], job_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Every job on a node (any node) in an active state, oldest first.
pub fn active_jobs(conn: &Connection) -> Vec<JobRow> {
    let list = ACTIVE_STATES.iter().map(|s| format!("'{s}'")).collect::<Vec<_>>().join(",");
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {JOB_COLS} FROM compute_jobs WHERE state IN ({list}) ORDER BY created_at, id")) else {
        return Vec::new();
    };
    stmt.query_map([], job_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn mark_notified(conn: &Connection, id: &str, at: i64) {
    let _ = conn.execute("UPDATE compute_jobs SET notified_at = ?2 WHERE id = ?1", params![id, at]);
}

/// The class and estimate a job carries into a shared node's queue (0215).
pub fn set_job_queue_facts(conn: &Connection, id: &str, class: &str, est_secs: Option<i64>) -> Result<(), String> {
    conn.execute("UPDATE compute_jobs SET class = ?2, est_secs = ?3 WHERE id = ?1", params![id, class, est_secs])
        .map_err(|e| e.to_string())
        .map(|_| ())
}

// ── job tiers (0230, prd-compute-job-tiers-v1) ───────────────────────

/// Non-terminal states, as SQL (queued here, on a node or running).
const OPEN_STATES_SQL: &str = "('queued','assigned','preparing','running','finishing')";

/// A queued job, wherever it waits: on this server, or in a shared node's
/// queue (`assigned` + `node_queued`).
const QUEUED_SQL: &str = "(state = 'queued' OR (state = 'assigned' AND node_queued = 1))";

/// A new job's tier facts: the tier the queue orders it by, what the
/// caller asked when the server lowered it, who sent it, its supersede key.
pub fn set_job_tier_facts(conn: &Connection, id: &str, tier: &str, asked: Option<&str>, submitter: Option<&str>, supersede_key: Option<&str>) -> Result<(), String> {
    conn.execute(
        "UPDATE compute_jobs SET tier = ?2, tier_asked = ?3, submitter = ?4, supersede_key = ?5 WHERE id = ?1",
        params![id, tier, asked, submitter, supersede_key],
    )
    .map_err(|e| e.to_string())
    .map(|_| ())
}

/// Who holds Priority on this server right now (prd §3.5): every submitter
/// with Priority work queued or running on any node, its workspace and how
/// many jobs, oldest holder first.
pub fn priority_holders(conn: &Connection) -> Vec<PriorityHolder> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT COALESCE(submitter, requested_by), workspace_id, COUNT(*), MIN(created_at) FROM compute_jobs \
         WHERE tier = 'priority' AND state IN {OPEN_STATES_SQL} GROUP BY 1, 2 ORDER BY 4, 1"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], |r| Ok(PriorityHolder { submitter: r.get(0)?, workspace_id: r.get(1)?, jobs: r.get(2)? }))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// One submitter on the critical path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorityHolder {
    pub submitter: String,
    pub workspace_id: String,
    pub jobs: i64,
}

/// `workspace_id`'s jobs with `key` that are still queued (here or in a
/// node's queue), oldest first: what a new `--supersede KEY` replaces.
pub fn queued_with_supersede_key(conn: &Connection, workspace_id: &str, key: &str) -> Vec<JobRow> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {JOB_COLS} FROM compute_jobs WHERE workspace_id = ?1 AND supersede_key = ?2 AND {QUEUED_SQL} ORDER BY created_at, id"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![workspace_id, key], job_from_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// How many of this server's jobs wait on `node_id`, by tier.
pub fn queued_tiers(conn: &Connection, node_id: &str) -> BTreeMap<String, i64> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT tier, COUNT(*) FROM compute_jobs WHERE node_id = ?1 AND {QUEUED_SQL} GROUP BY tier")) else {
        return BTreeMap::new();
    };
    stmt.query_map(params![node_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

/// End a job still queued on this server as replaced by `by_job` (a newer
/// `--supersede` with the same key). `Ok(false)` when it isn't queued here
/// any more (assigned, started or ended meanwhile): nothing changed.
pub fn replace_if_queued(conn: &Connection, id: &str, by_job: &str, at: i64) -> Result<bool, String> {
    let detail = format!("replaced by job {}", super::message::short(by_job));
    set_job_state_if(conn, id, "queued", JobState::Cancelled, Some(super::proto::tiers::codes::REPLACED), Some(&detail), at)
}

/// The node holds attempt `generation` in its queue: keep the row
/// `assigned`, mark it node-queued and mirror the position (and, when
/// given, set the reason). Only while that attempt is still `assigned`
/// here (a cancel or a newer attempt wins). `Ok(true)` when marked.
pub fn mark_node_queued(conn: &Connection, id: &str, generation: i64, pos: Option<&QueuePos>, reason: Option<&str>, at: i64) -> Result<bool, String> {
    conn.execute(
        "UPDATE compute_jobs SET node_queued = 1, reason = COALESCE(?8, reason), node_pos = ?3, node_eta_secs = ?4, \
         node_eta_p90_secs = ?5, node_blocked_by = ?6, node_seen_at = ?7, node_of = ?9 WHERE id = ?1 AND generation = ?2 AND state = 'assigned'",
        params![
            id,
            generation,
            pos.map(|p| p.pos as i64),
            pos.and_then(|p| p.eta_secs).map(|e| e as i64),
            pos.and_then(|p| p.eta_p90_secs).map(|e| e as i64),
            pos.and_then(|p| p.blocked_by.clone()),
            at,
            reason,
            pos.and_then(|p| p.of).map(i64::from)
        ],
    )
    .map_err(|e| e.to_string())
    .map(|n| n == 1)
}

/// The node started (or ended) a node-queued job: it's no longer queued.
pub fn clear_node_queued(conn: &Connection, id: &str) {
    let _ = conn.execute(
        "UPDATE compute_jobs SET node_queued = 0, node_pos = NULL, node_eta_secs = NULL, node_eta_p90_secs = NULL, \
         node_blocked_by = NULL, node_of = NULL WHERE id = ?1 AND node_queued = 1",
        params![id],
    );
}

/// A node-queued job comes back to this server under a NEW generation
/// (a confirmed withdraw, V46; a lost node, V56): back to `queued`, same
/// attempt, no retry used. The old `(job, generation)` can never run or
/// report again, so a later assign to the same node runs it. Only from an
/// active state of that generation (a cancel that ended it, or a newer
/// attempt, wins). `Ok(true)` when requeued.
pub fn requeue_new_generation(conn: &Connection, id: &str, generation: i64, reason: &str) -> Result<bool, String> {
    conn.execute(
        "UPDATE compute_jobs SET state = 'queued', reason = ?3, detail = NULL, generation = generation + 1, \
         assigned_at = NULL, started_at = NULL, log_seq = 0, log_bytes = 0, node_queued = 0, node_pos = NULL, \
         node_eta_secs = NULL, node_eta_p90_secs = NULL, node_blocked_by = NULL, node_of = NULL \
         WHERE id = ?1 AND generation = ?2 AND state IN ('assigned','preparing','running','finishing')",
        params![id, generation, reason],
    )
    .map_err(|e| e.to_string())
    .map(|n| n == 1)
}

/// The mover asked the node to give this job back (V71's limit).
pub fn note_withdraw(conn: &Connection, id: &str, at: i64) {
    let _ = conn.execute("UPDATE compute_jobs SET withdraws = withdraws + 1, withdrawn_at = ?2 WHERE id = ?1", params![id, at]);
}

/// Every job a node holds in its queue for this server (all nodes).
pub fn node_queued_jobs(conn: &Connection) -> Vec<JobRow> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {JOB_COLS} FROM compute_jobs WHERE node_queued = 1 AND state = 'assigned' ORDER BY created_at, id"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], job_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// A new attempt: generation + 1, back to queued, results cleared.
pub fn requeue_new_attempt(conn: &Connection, id: &str, reason: &str) -> Result<JobRow, String> {
    conn.execute(
        "UPDATE compute_jobs SET state = 'queued', reason = ?2, detail = NULL, attempt = attempt + 1, \
         generation = generation + 1, exit_code = NULL, signal = NULL, assigned_at = NULL, started_at = NULL, \
         ended_at = NULL, log_seq = 0, log_bytes = 0, summary_json = NULL, receipt_json = NULL, receipt_ok = NULL, \
         notified_at = NULL, node_queued = 0, node_pos = NULL, node_eta_secs = NULL, node_eta_p90_secs = NULL, \
         node_blocked_by = NULL, node_of = NULL WHERE id = ?1",
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

/// Who did `kind` to `job_id` last, and when (the audit line `get` shows
/// for a cancel).
pub fn last_job_event(conn: &Connection, job_id: &str, kind: &str) -> Option<(String, i64)> {
    conn.query_row(
        "SELECT actor, at FROM compute_events WHERE job_id = ?1 AND kind = ?2 ORDER BY id DESC LIMIT 1",
        params![job_id, kind],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// The event kinds a job's own stream shows (`k2 compute events`,
/// prd-compute-easy-mode-0467 feedback part 2 item 4).
pub const JOB_STREAM_KINDS: &[&str] = &[
    "job_submit",
    "job_start",
    "job_end",
    "job_queue_timeout",
    "job_unknown",
    "job_lost",
    "job_cancel",
    "job_retry",
    "job_requeue",
    "job_move",
];

/// Job events after `after` (an event id; negative = the newest `limit`),
/// oldest first: only the kinds in
/// [`JOB_STREAM_KINDS`], only jobs of `workspace_id` (when given), of
/// `session_id` (when given) and with `tag` (when given). Joined with the
/// job row, so a job deleted since drops out.
pub fn job_events_after(
    conn: &Connection,
    workspace_id: Option<&str>,
    session_id: Option<&str>,
    tag: Option<&str>,
    after: i64,
    limit: i64,
) -> Vec<EventRow> {
    let kinds = JOB_STREAM_KINDS.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",");
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT e.id, e.at, e.actor, e.kind, e.node_id, j.workspace_id, e.job_id, e.detail_json FROM compute_events e \
         JOIN compute_jobs j ON j.id = e.job_id WHERE e.id > ?1 AND e.kind IN ({kinds}) \
         AND (?2 IS NULL OR j.workspace_id = ?2) AND (?3 IS NULL OR j.session_id = ?3) \
         AND (?4 IS NULL OR json_extract(j.plan_json, '$.k2ctl.tag') = ?4) ORDER BY e.id {} LIMIT ?5",
        if after < 0 { "DESC" } else { "ASC" }
    )) else {
        return Vec::new();
    };
    let rows = stmt.query_map(params![after, workspace_id, session_id, tag, limit.clamp(1, 500)], |r| {
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
    });
    let mut out: Vec<EventRow> = rows.map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default();
    if after < 0 {
        out.reverse();
    }
    out
}

/// The newest event id (0 when none): where a stream that starts "now"
/// begins.
pub fn last_event_id(conn: &Connection) -> i64 {
    conn.query_row("SELECT COALESCE(MAX(id), 0) FROM compute_events", [], |r| r.get(0)).unwrap_or(0)
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
            class: None,
            session_id: None,
            est_secs: None,
            est_p90_secs: None,
            ws_max_running: None,
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
            needs: Vec::new(),
            artifacts: Vec::new(),
            stall_secs: None,
            secret_env: Vec::new(),
            tier: None,
            cache: None,
            src_path: None, submitter: None, submitter_max_running: None,
            with: Vec::new(),
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
            &NewJob { plan: &p, client_job_id: "client-0000000001", session_id: Some("s1"), request_sha256: "h", detach: true, retry_interrupted: true, ctl: JobCtl::default() },
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
            &NewJob { plan: &plan("other", "n1", "ws"), client_job_id: "client-0000000001", session_id: None, request_sha256: "x", detach: false, retry_interrupted: false, ctl: JobCtl::default() }
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

    /// V46/V56: a node-queued job comes back under a NEW generation with
    /// the same attempt (no retry used), only from its own active attempt.
    #[test]
    fn node_queued_jobs_requeue_under_a_new_generation() {
        let c = conn();
        let p = plan("nq00000000000000000000000000000001", "n1", "ws");
        let j = insert_job(
            &c,
            &NewJob { plan: &p, client_job_id: "client-nq-000000001", session_id: None, request_sha256: "h", detach: false, retry_interrupted: true, ctl: JobCtl::default() },
        )
        .unwrap();
        assert_eq!((j.class.as_str(), j.node_queued, j.withdraws), ("interactive", false, 0), "0215 defaults");
        set_job_queue_facts(&c, &j.id, "batch", Some(90)).unwrap();
        assert!(!mark_node_queued(&c, &j.id, 1, None, Some("node_queued"), 5).unwrap(), "only an assigned row");
        assert!(assign_if_queued(&c, &j.id, 5).unwrap());
        let pos = QueuePos { pos: 2, of: None, eta_secs: Some(360), eta_p90_secs: Some(600), blocked_by: Some("over_quota".into()) };
        assert!(!mark_node_queued(&c, &j.id, 9, Some(&pos), None, 6).unwrap(), "another generation");
        assert!(mark_node_queued(&c, &j.id, 1, Some(&pos), Some("node_queued"), 6).unwrap());
        let r = job_by_id(&c, &j.id).unwrap();
        assert_eq!((r.state.as_str(), r.reason.as_deref(), r.node_queued, r.node_pos, r.node_eta_secs), ("assigned", Some("node_queued"), true, Some(2), Some(360)));
        assert_eq!((r.class.as_str(), r.est_secs, r.node_blocked_by.as_deref()), ("batch", Some(90), Some("over_quota")));
        assert_eq!(node_queued_jobs(&c).len(), 1);
        assert_eq!(queued_count(&c, "n1", "ws"), 1, "a node-queued job counts against the grant's max_queued");
        assert_eq!(list_jobs(&c, &JobFilter { state: Some("queued"), ..Default::default() }).len(), 1, "and lists as queued");
        note_withdraw(&c, &j.id, 7);
        assert!(!requeue_new_generation(&c, &j.id, 2, "withdrawn").unwrap(), "a stale generation never requeues");
        assert!(requeue_new_generation(&c, &j.id, 1, "withdrawn").unwrap());
        let r = job_by_id(&c, &j.id).unwrap();
        assert_eq!((r.state.as_str(), r.attempt, r.generation, r.node_queued, r.node_pos), ("queued", 1, 2, false, None));
        assert_eq!((r.withdraws, r.withdrawn_at), (1, Some(7)));
        assert!(node_queued_jobs(&c).is_empty());
        // A cancelled job never comes back.
        set_job_state(&c, &j.id, JobState::Cancelled, Some("cancelled"), None, 8).unwrap();
        assert!(!requeue_new_generation(&c, &j.id, 2, "withdrawn").unwrap());
        // Started: no longer node-queued.
        let k = insert_job(
            &c,
            &NewJob { plan: &plan("nq00000000000000000000000000000002", "n1", "ws"), client_job_id: "client-nq-000000002", session_id: None, request_sha256: "h", detach: false, retry_interrupted: true, ctl: JobCtl::default() },
        )
        .unwrap();
        assert!(assign_if_queued(&c, &k.id, 5).unwrap());
        assert!(mark_node_queued(&c, &k.id, 1, None, Some("node_queued"), 6).unwrap());
        clear_node_queued(&c, &k.id);
        assert!(!job_by_id(&c, &k.id).unwrap().node_queued);
    }

    #[test]
    fn share_mirror_and_node_servers_round_trip() {
        use crate::compute::proto::share::{Role, ServerStatus, ShareLimits, Trust};
        let c = conn();
        let n = insert_pending_node(&c, "n1", "mini-1", "fp1", "pem", &labels(), "owner", "1", 1).unwrap();
        assert_eq!((n.role.as_str(), n.shared, n.share.is_none()), ("owner", false, true), "every node starts as owner");
        let v = ShareView {
            role: Role::Tenant,
            slot: 1,
            shared: true,
            servers: 2,
            owner_label: Some("rosson.k2.dev".into()),
            state: ServerStatus::Active,
            limits: ShareLimits::new_server(Trust::Team, false),
            running: 0,
            queued: 1,
            cpu_secs_today: 7,
            expires_in_secs: Some(100),
            skew_secs: None,
            quotes: vec![],
            priority: None,
        };
        store_share(&c, "n1", Some(&v), 50);
        let n = node_by_id(&c, "n1").unwrap();
        assert!(n.is_tenant() && n.shared);
        assert_eq!((n.owner_label.as_deref(), n.share.as_ref(), n.share_at), (Some("rosson.k2.dev"), Some(&v), Some(50)));
        store_share(&c, "n1", None, 60);
        let n = node_by_id(&c, "n1").unwrap();
        assert!(n.is_tenant(), "a tenant stays a tenant");
        assert!(!n.shared && n.share.is_none());
        assert!(serde_json::to_value(&n).unwrap().get("role").is_none(), "solo JSON is unchanged");
        // Review A-H1: the node's own offer can't lift it back to owner.
        let mut lie = v.clone();
        lie.role = Role::Owner;
        lie.owner_label = None;
        store_share(&c, "n1", Some(&lie), 70);
        let n = node_by_id(&c, "n1").unwrap();
        assert!(n.is_tenant() && !n.owned_for_secrets(), "an offer never turns a tenant into owned");
        // Only the confirmed owner-change hook does.
        mark_node_owned(&c, "n1").unwrap();
        let n = node_by_id(&c, "n1").unwrap();
        assert!(!n.is_tenant() && n.owned_for_secrets());
        let owned = insert_pending_node(&c, "n9", "mini-9", "fp9", "pem", &labels(), "owner", "1", 1).unwrap();
        assert!(owned.owned_for_secrets(), "a node this server enrolled is its own");
        // Owner side: the node's server list.
        let rows = vec![
            crate::compute::share::ServerRow {
                slot: 0,
                label: "rosson.k2.dev".into(),
                fp: Some("ab".repeat(32)),
                state: "active".into(),
                trust: "own".into(),
                limits: ShareLimits::owner(2, 8, 16, 100, crate::compute::proto::share::Network::On),
                usage: None,
                expires_at: None,
                added_at: Some(1),
                added_by: Some("local:rosson".into()),
            },
            crate::compute::share::ServerRow {
                slot: 1,
                label: "baden.k2.dev".into(),
                fp: Some("0123456789abcdef".into()),
                state: "pending".into(),
                trust: "team".into(),
                limits: ShareLimits::new_server(Trust::Team, false),
                usage: Some(serde_json::json!({"sas": "482913"})),
                expires_at: Some(99),
                added_at: Some(2),
                added_by: Some("owner-approval:car_x".into()),
            },
        ];
        replace_node_servers(&c, "n1", &rows, 70).unwrap();
        assert_eq!(node_servers(&c, "n1"), rows);
        assert_eq!(node_servers_seen_at(&c, "n1"), Some(70));
        replace_node_servers(&c, "n1", &rows[..1], 80).unwrap();
        assert_eq!(node_servers(&c, "n1").len(), 1, "a server gone from the list is gone from the mirror");
        let bad = crate::compute::share::ServerRow { slot: 9, ..rows[1].clone() };
        assert!(replace_node_servers(&c, "n1", &[bad], 90).is_err(), "slot 0–7 is checked by the table");
        assert_eq!(node_servers(&c, "n1").len(), 1, "a failed refresh keeps the last good list");
    }

    /// Security review (cross-branch): a tenant row never turns owned by
    /// anything its node says: not a share view saying `owner`, not an
    /// offer with no view and owner-only features (the old MT36 path). It
    /// is only a claim until `mark_node_owned` (the person's take-over), which
    /// also clears the owner label.
    #[test]
    fn a_tenant_row_never_turns_owned_by_an_offer() {
        use crate::compute::proto::frames::{Availability, Caps, Control, Free};
        use crate::compute::proto::share::{Role, ServerStatus, ShareLimits, Trust};
        let c = conn();
        insert_pending_node(&c, "n1", "mini-1", "fp1", "pem", &labels(), "owner", "1", 1).unwrap();
        confirm_node(&c, "n1", "owner", 2).unwrap();
        set_tenant_of(&c, "n1", "rosson.k2.dev").unwrap();
        let view = |role: Role| ShareView {
            role,
            slot: 0,
            shared: false,
            servers: 1,
            owner_label: None,
            state: ServerStatus::Active,
            limits: ShareLimits::new_server(Trust::Own, false),
            running: 0,
            queued: 0,
            cpu_secs_today: 0,
            expires_in_secs: None,
            skew_secs: None,
            quotes: vec![],
            priority: None,
        };
        let offer = |share: Option<ShareView>, features: &[&str]| Offer {
            share,
            revision: 1,
            control: Control::Active,
            control_by: None,
            control_at: None,
            availability: Availability { ok: true, reasons: vec![] },
            caps: Caps { cpu_millis: 1000, mem_bytes: 1 << 30, disk_budget_bytes: 1 << 30, hard: true },
            free: Free { cpu_millis: 1000, mem_bytes: 1 << 30, disk_free_bytes: 1 << 30 },
            slots: vec![],
            running: 0,
            parallel_max: 1,
            refusal: None,
            labels: Default::default(),
            tools: Default::default(),
            protocol: crate::compute::proto::PROTOCOL,
            boot_id: "b".into(),
            ledger_id: "l".into(),
            node_version: "0.46.4".into(),
            features: features.iter().map(|f| f.to_string()).collect(),
        };
        let owner_feats = ["queue", "servers", "admin", "health", "self-update"];
        assert!(!node_by_id(&c, "n1").unwrap().owner_claim(), "no offer yet: no claim");
        for o in [offer(Some(view(Role::Owner)), &owner_feats), offer(None, &owner_feats)] {
            store_offer(&c, "n1", &o, 10);
            store_share(&c, "n1", o.share.as_ref(), 10);
            let n = node_by_id(&c, "n1").unwrap();
            assert!(n.is_tenant(), "the offer can't lift a tenant: {:?}", o.share);
            assert!(!n.owned_for_secrets());
            assert_eq!(n.owner_label.as_deref(), Some("rosson.k2.dev"));
            assert!(n.owner_claim(), "it's a claim, for the person to confirm");
        }
        // A plain tenant offer is no claim.
        let o = offer(Some(view(Role::Tenant)), &["queue", "servers"]);
        store_offer(&c, "n1", &o, 11);
        store_share(&c, "n1", o.share.as_ref(), 11);
        assert!(!node_by_id(&c, "n1").unwrap().owner_claim());
        // The person's take-over: owned, the label gone.
        mark_node_owned(&c, "n1").unwrap();
        let n = node_by_id(&c, "n1").unwrap();
        assert!(!n.is_tenant() && n.owned_for_secrets() && n.owner_label.is_none());
        assert!(!n.owner_claim(), "an owned node makes no claim");
        // Owned → tenant by an offer is still allowed (one way).
        let mut v = view(Role::Tenant);
        v.owner_label = Some("z3thon.k2.dev".into());
        store_share(&c, "n1", Some(&v), 12);
        let n = node_by_id(&c, "n1").unwrap();
        assert!(n.is_tenant() && !n.owned_for_secrets());
    }

    #[test]
    fn ctl_rides_in_plan_json_outside_the_plan_and_moves_rebuild_the_digest() {
        let c = conn();
        let p = plan("job-ctl-0000000000000000000000001", "n1", "ws");
        let ctl = JobCtl {
            any: Some(AnyFilter { os: Some("linux".into()), ..Default::default() }),
            queue_timeout_secs: Some(1800),
            queued_at: 10,
            retries: 1,
            ..Default::default()
        };
        let j = insert_job(
            &c,
            &NewJob { plan: &p, client_job_id: "client-ctl-000000001", session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: ctl.clone() },
        )
        .unwrap();
        assert_eq!(j.ctl, ctl);
        assert_eq!(j.plan().unwrap(), p, "the plan reads back without the ctl key");
        assert_eq!(j.plan_digest, p.digest().unwrap());
        assert_eq!(queued_jobs(&c).len(), 1);
        // A requeue keeps the options.
        let mut more = ctl.clone();
        more.retries_used = 1;
        more.failed_attempts.push(AttemptNote { attempt: 1, node_id: "n1".into(), state: "interrupted".into(), reason: Some("node_lost".into()), at: 20 });
        set_job_ctl(&c, &j.id, &more).unwrap();
        let r = requeue_new_attempt(&c, &j.id, "retry").unwrap();
        assert_eq!(r.ctl, more);
        // A move rewrites node, plan and digest, only while queued.
        let mut q = p.clone();
        q.node_id = "n2".into();
        assert!(move_queued_job(&c, &j.id, &q, &more, "moved").unwrap());
        let m = job_by_id(&c, &j.id).unwrap();
        assert_eq!((m.node_id.as_str(), m.plan_digest.clone(), m.ctl.clone()), ("n2", q.digest().unwrap(), more.clone()));
        set_job_state(&c, &j.id, JobState::Assigned, None, None, 30).unwrap();
        assert!(!move_queued_job(&c, &j.id, &p, &more, "moved").unwrap(), "never moves an assigned job");
        assert_eq!(active_jobs(&c).len(), 1);
    }

    #[test]
    fn durations_history_and_quantiles() {
        let c = conn();
        for (i, secs) in [(1, 60), (2, 80), (3, 100), (4, 300)] {
            let p = plan(&format!("dur-{i:0>28}"), if i == 4 { "n2" } else { "n1" }, "ws");
            insert_job(
                &c,
                &NewJob { plan: &p, client_job_id: &format!("client-dur-{i:0>8}"), session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: JobCtl::default() },
            )
            .unwrap();
            set_job_state(&c, &p.job_id, JobState::Running, None, None, 1000).unwrap();
            set_job_state(&c, &p.job_id, JobState::Done, None, None, 1000 + secs).unwrap();
        }
        let argv = vec!["cargo".to_string(), "test".to_string()];
        let mut on_n1 = recent_durations(&c, "ws", &argv, Some("n1"), 20);
        on_n1.sort_unstable();
        assert_eq!(on_n1, vec![60, 80, 100]);
        assert_eq!(recent_durations(&c, "ws", &argv, None, 20).len(), 4);
        assert!(recent_durations(&c, "ws", &["other".to_string()], None, 20).is_empty());
        assert_eq!(median_p90(&[60, 80, 100]), Some((80, 100)));
        assert_eq!(median_p90(&[5]), Some((5, 5)));
        assert_eq!(median_p90(&[]), None);
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

    fn queued_job(c: &Connection, id: &str, client: &str) -> JobRow {
        insert_job(
            c,
            &NewJob { plan: &plan(id, "n1", "ws"), client_job_id: client, session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: Default::default() },
        )
        .expect("insert job")
    }

    #[test]
    fn cancel_if_queued_does_not_touch_an_assigned_job() {
        let c = conn();
        let a = queued_job(&c, "aaaaaaaa0123456789abcdef01234567", "client-000000000a");
        assert!(assign_if_queued(&c, &a.id, 20).expect("assign"));
        assert!(!cancel_if_queued(&c, &a.id, 21).expect("cancel"), "an assigned job is not cancelled by the queued-only path");
        let row = job_by_id(&c, &a.id).expect("row");
        assert_eq!((row.state.as_str(), row.assigned_at, row.ended_at), ("assigned", Some(20), None));
        // A queued one is cancelled once, with an end time.
        let b = queued_job(&c, "bbbbbbbb0123456789abcdef01234567", "client-000000000b");
        assert!(cancel_if_queued(&c, &b.id, 30).expect("cancel"));
        assert!(!cancel_if_queued(&c, &b.id, 31).expect("cancel again"));
        let row = job_by_id(&c, &b.id).expect("row");
        assert_eq!((row.state.as_str(), row.reason.as_deref(), row.ended_at), ("cancelled", Some("cancelled"), Some(30)));
    }

    #[test]
    fn assign_if_queued_never_revives_a_cancelled_job() {
        let c = conn();
        let a = queued_job(&c, "cccccccc0123456789abcdef01234567", "client-000000000c");
        assert!(cancel_if_queued(&c, &a.id, 20).expect("cancel"));
        assert!(!assign_if_queued(&c, &a.id, 21).expect("assign"), "kick must not assign a job a cancel already ended");
        assert_eq!(job_by_id(&c, &a.id).expect("row").state, "cancelled");
        assert!(!set_job_state_if(&c, &a.id, "queued", JobState::Failed, Some("x"), None, 22).expect("fail"));
        assert_eq!(job_by_id(&c, &a.id).expect("row").state, "cancelled");
    }

    // ── job tiers (0230) ─────────────────────────────────────────────

    /// T1: `allow_ci` is off for a new grant, set only when asked, and an
    /// absent `allow_ci` (re-granting limits) keeps what the grant had.
    #[test]
    fn allow_ci_is_off_by_default_and_kept_when_absent() {
        let c = conn();
        insert_pending_node(&c, "n1", "a", "fp", "pem", &labels(), "o", "1", 1).unwrap();
        let g = set_grant(&c, "n1", "ws", &GrantLimits::default(), "owner", 2).unwrap();
        assert!(!g.allow_ci);
        let g = set_grant(&c, "n1", "ws", &GrantLimits { allow_ci: Some(true), ..Default::default() }, "owner", 3).unwrap();
        assert!(g.allow_ci);
        let g = set_grant(&c, "n1", "ws", &GrantLimits { max_parallel: Some(2), ..Default::default() }, "owner", 4).unwrap();
        assert!(g.allow_ci, "re-granting limits never drops CI");
        assert_eq!(g.max_parallel, 2);
        let g = set_grant(&c, "n1", "ws", &GrantLimits { allow_ci: Some(false), ..Default::default() }, "owner", 5).unwrap();
        assert!(!g.allow_ci);
        assert_eq!(serde_json::to_value(&g).unwrap()["allowCi"], serde_json::json!(false), "the routes show it as allowCi");
    }

    fn tiered_job(c: &Connection, id: &str, ws: &str, tier: &str, submitter: &str, key: Option<&str>, at: i64) -> JobRow {
        let mut p = plan(id, "n1", ws);
        p.created_at = at;
        let j = insert_job(
            c,
            &NewJob { plan: &p, client_job_id: &format!("client-{id}"), session_id: None, request_sha256: "h", detach: false, retry_interrupted: false, ctl: JobCtl::default() },
        )
        .unwrap();
        set_job_tier_facts(c, &j.id, tier, None, Some(submitter), key).unwrap();
        job_by_id(c, &j.id).unwrap()
    }

    /// The critical-path note (§3.5) reads open Priority work by submitter;
    /// ended jobs and other tiers don't count.
    #[test]
    fn priority_holders_count_open_priority_work_by_submitter() {
        let c = conn();
        assert!(priority_holders(&c).is_empty());
        let a = tiered_job(&c, "p1000000000000000000000000000001", "ws", "priority", "k2/7f3a/.claude/worktrees/builder-a", None, 1);
        tiered_job(&c, "p1000000000000000000000000000002", "ws", "priority", "k2/7f3a/.claude/worktrees/builder-a", None, 2);
        tiered_job(&c, "p1000000000000000000000000000003", "ws", "normal", "k2/9b1c", None, 3);
        let b = tiered_job(&c, "p1000000000000000000000000000004", "ws2", "priority", "ops/1d2e", None, 4);
        assert_eq!(a.tier(), crate::compute::proto::tiers::Tier::Priority);
        let h = priority_holders(&c);
        assert_eq!(h.iter().map(|x| (x.submitter.as_str(), x.jobs)).collect::<Vec<_>>(), vec![("k2/7f3a/.claude/worktrees/builder-a", 2), ("ops/1d2e", 1)]);
        set_job_state(&c, &b.id, JobState::Done, None, None, 9).unwrap();
        assert_eq!(priority_holders(&c).len(), 1, "an ended job no longer holds the critical path");
        let mut tiers = queued_tiers(&c, "n1");
        assert_eq!((tiers.remove("priority"), tiers.remove("normal")), (Some(2), Some(1)));
        assert_eq!(list_jobs(&c, &JobFilter { tier: Some("priority"), ..Default::default() }).len(), 3);
    }

    /// Supersede (§3.4): only this workspace's still-queued jobs with the
    /// key; a replaced one ends `cancelled/replaced`; a started one is
    /// never touched (R3).
    #[test]
    fn supersede_finds_only_queued_jobs_with_the_key_and_replaces_them() {
        let c = conn();
        let old = tiered_job(&c, "s1000000000000000000000000000001", "ws", "ci", "ci", Some("ci/main/rust"), 1);
        let started = tiered_job(&c, "s1000000000000000000000000000002", "ws", "ci", "ci", Some("ci/main/rust"), 2);
        tiered_job(&c, "s1000000000000000000000000000003", "ws", "ci", "ci", Some("ci/main/ui"), 3);
        tiered_job(&c, "s1000000000000000000000000000004", "other", "ci", "ci", Some("ci/main/rust"), 4);
        set_job_state(&c, &started.id, JobState::Running, None, None, 5).unwrap();
        let found = queued_with_supersede_key(&c, "ws", "ci/main/rust");
        assert_eq!(found.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(), vec![old.id.as_str()]);
        assert!(replace_if_queued(&c, &old.id, "newjob00000000000000000000000001", 6).unwrap());
        let r = job_by_id(&c, &old.id).unwrap();
        assert_eq!((r.state.as_str(), r.reason.as_deref()), ("cancelled", Some("replaced")));
        assert!(r.detail.as_deref().is_some_and(|d| d.starts_with("replaced by job ")), "{:?}", r.detail);
        assert!(!replace_if_queued(&c, &started.id, "newjob00000000000000000000000001", 7).unwrap(), "a started job is never replaced");
        assert_eq!(job_by_id(&c, &started.id).unwrap().state, "running");
    }
}
