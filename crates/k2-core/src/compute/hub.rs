//! The compute hub (`prd-compute-shared-queue-v1` S1/S2): one server owns
//! the nodes and the queue; other K2 servers submit to it as **tenants**.
//! This module is the pure + SQLite half (the daemon's `compute_hub` /
//! `compute_hub_client` hold the routes, the HTTP client and the poller).
//!
//! **Tenants (migration 0204).** `local` is this server's own workspaces
//! (today's passport path, unchanged). A `server` tenant is another K2
//! server bound to its federation fingerprint: paired with pairing v2,
//! Trusted, and granted the peer capability [`CAP_COMPUTE`]. Until the
//! runtime wall lands (S6) only `own` tenants are accepted.
//!
//! **Signed requests (V9).** Every tenant call to `/cli/compute/hub/*`
//! carries [`HEADER`]: `fp=<hex>, ts=<unix>, nonce=<hex>, epoch=<n>,
//! sig=<b64>`. The signature covers, under the domain [`DOMAIN`], the
//! method, path, raw query, sha256(body), fingerprint, ts, nonce and the
//! tenant epoch. The hub checks skew (300 s), the pinned key, the peer
//! gate (`require_peer(fp, "compute")`, pairing v2), the tenant's state and
//! epoch (V25), and a per-tenant nonce cache (replays refused).
//!
//! **Job keys (V28, V30).** A tenant job's `workspace_id` on the hub is
//! [`tenant_ws_key`] = `t/<tenant>/<remote workspace id>`, so every
//! per-workspace rule the controller already has (fair queue, grants,
//! queue caps, durations) is tenant-scoped by construction. Its mirror and
//! warm slots are named by [`hub_project_key`], derived from the tenant id,
//! never from an id the submitter chose alone.
//!
//! For the builders of S3 (fair queue v2), S4 (reservations) and S7 (UI):
//! - tenant rows: [`tenant_by_id`], [`tenant_by_fp`], [`list_tenants`],
//!   [`TenantRow`] (`weight`, `top_class`, `max_running`, `max_queued`,
//!   `cpu_hours_day`, `log_gb`, `artifact_days` are stored and validated
//!   here; S3 reads them);
//! - a job's tenant: `compute_jobs.tenant_id` (`JobRow::tenant_id`), its
//!   class (`JobRow::class`: gate | interactive | batch);
//! - per-node tenant limits: [`tenant_grant`] (also folded into
//!   `store::grant` for `t/…` keys, which is what placement reads).

use std::collections::HashMap;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Serialize;

use super::proto::crypto;

/// The header a tenant's signed hub request carries.
pub const HEADER: &str = "X-K2-Hub";
/// Domain separation for the hub request signature.
pub const DOMAIN: &str = "k2-compute-hub-v1";
/// The federation peer capability a server tenant needs.
pub const CAP_COMPUTE: &str = "compute";
/// Accepted clock skew (seconds), as the roster GET.
pub const SKEW_SECS: i64 = 300;
/// The `local` tenant (this server's own workspaces).
pub const LOCAL: &str = "local";
/// Remembered nonces per tenant (older ones are past the skew anyway).
const NONCE_CAP: usize = 4096;

/// Default blob store size per tenant when `blob_mb` is NULL (5 GiB).
pub const DEFAULT_BLOB_MB: i64 = 5 * 1024;
/// Largest single blob over the relay (V21; the relay bundle cap).
pub const RELAY_MAX_BLOB_BYTES: u64 = 50 << 20;
/// Largest single blob on a LAN / loopback door (V21).
pub const LAN_MAX_BLOB_BYTES: u64 = 1 << 30;
/// Logs kept per tenant (Rosson 2026-10-20, Q12).
pub const LOG_KEEP_DAYS: i64 = 14;
/// Artifacts and blobs kept per tenant (Q12).
pub const ARTIFACT_KEEP_DAYS: i64 = 7;

/// Stable refusal codes for the hub routes (each its own code, §11).
pub mod codes {
    pub const AUTH_MISSING: &str = "hub_auth_missing";
    pub const SKEW: &str = "hub_skew";
    pub const BAD_SIGNATURE: &str = "hub_bad_signature";
    pub const REPLAY: &str = "hub_replay";
    pub const TENANT_UNKNOWN: &str = "tenant_unknown";
    pub const PEER_NOT_GRANTED: &str = "peer_not_granted";
    pub const PAIRING_V2_REQUIRED: &str = "pairing_v2_required";
    pub const TENANT_SUSPENDED: &str = "tenant_suspended";
    pub const TENANT_REVOKED: &str = "tenant_revoked";
    pub const EPOCH_STALE: &str = "tenant_epoch_stale";
    pub const HUB_UNREACHABLE: &str = "hub_unreachable";
    pub const WALL_REQUIRED: &str = "wall_required";
    pub const CLASS_NOT_ALLOWED: &str = "class_not_allowed";
    pub const BLOB_TOO_LARGE: &str = "blob_too_large";
    pub const BLOB_QUOTA: &str = "blob_quota";
    pub const BLOB_MISMATCH: &str = "blob_mismatch";
    pub const SOURCE_MISSING: &str = "source_missing";
}

// ── tenants ──────────────────────────────────────────────────────────

/// One tenant row.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantRow {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub peer_fp: Option<String>,
    pub trust: String,
    pub weight: i64,
    pub top_class: String,
    pub max_running: Option<i64>,
    pub max_queued: Option<i64>,
    pub cpu_hours_day: Option<i64>,
    pub log_gb: Option<i64>,
    pub artifact_days: Option<i64>,
    pub blob_mb: Option<i64>,
    pub state: String,
    pub epoch: i64,
    pub created_by: Option<String>,
    pub created_at: Option<i64>,
}

impl TenantRow {
    pub fn is_local(&self) -> bool {
        self.id == LOCAL
    }
    /// Blob store cap in bytes.
    pub fn blob_cap_bytes(&self) -> u64 {
        (self.blob_mb.unwrap_or(DEFAULT_BLOB_MB).max(1) as u64) << 20
    }
    /// May this tenant submit a job of `class`? Classes order
    /// gate > interactive > batch; a tenant may use its top class and below.
    pub fn allows_class(&self, class: &str) -> bool {
        class_rank(class).is_some_and(|c| class_rank(&self.top_class).is_some_and(|t| c <= t))
    }
}

/// gate 2, interactive 1, batch 0; None for anything else.
pub fn class_rank(class: &str) -> Option<u8> {
    match class {
        "gate" => Some(2),
        "interactive" => Some(1),
        "batch" => Some(0),
        _ => None,
    }
}

const TENANT_COLS: &str = "id, kind, label, peer_fp, trust, weight, top_class, max_running, max_queued, cpu_hours_day, \
    log_gb, artifact_days, blob_mb, state, epoch, created_by, created_at";

fn tenant_from_row(r: &Row<'_>) -> rusqlite::Result<TenantRow> {
    Ok(TenantRow {
        id: r.get(0)?,
        kind: r.get(1)?,
        label: r.get(2)?,
        peer_fp: r.get(3)?,
        trust: r.get(4)?,
        weight: r.get(5)?,
        top_class: r.get(6)?,
        max_running: r.get(7)?,
        max_queued: r.get(8)?,
        cpu_hours_day: r.get(9)?,
        log_gb: r.get(10)?,
        artifact_days: r.get(11)?,
        blob_mb: r.get(12)?,
        state: r.get(13)?,
        epoch: r.get(14)?,
        created_by: r.get(15)?,
        created_at: r.get(16)?,
    })
}

pub fn tenant_by_id(conn: &Connection, id: &str) -> Option<TenantRow> {
    conn.query_row(&format!("SELECT {TENANT_COLS} FROM compute_tenants WHERE id = ?1"), params![id], tenant_from_row)
        .optional()
        .ok()
        .flatten()
}

pub fn tenant_by_fp(conn: &Connection, fp: &str) -> Option<TenantRow> {
    conn.query_row(&format!("SELECT {TENANT_COLS} FROM compute_tenants WHERE peer_fp = ?1"), params![fp], tenant_from_row)
        .optional()
        .ok()
        .flatten()
}

/// A tenant by id or label (labels are unique among non-revoked rows by
/// convention; the id wins).
pub fn find_tenant(conn: &Connection, token: &str) -> Option<TenantRow> {
    let t = token.trim();
    tenant_by_id(conn, t).or_else(|| {
        conn.query_row(
            &format!("SELECT {TENANT_COLS} FROM compute_tenants WHERE label = ?1 AND state != 'revoked' ORDER BY created_at DESC LIMIT 1"),
            params![t],
            tenant_from_row,
        )
        .optional()
        .ok()
        .flatten()
    })
}

pub fn list_tenants(conn: &Connection) -> Vec<TenantRow> {
    let Ok(mut stmt) = conn.prepare(&format!("SELECT {TENANT_COLS} FROM compute_tenants ORDER BY id = 'local' DESC, label")) else {
        return Vec::new();
    };
    stmt.query_map([], tenant_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

/// Everything a new server tenant needs.
#[derive(Debug, Clone)]
pub struct NewTenant<'a> {
    pub label: &'a str,
    pub peer_fp: &'a str,
    pub trust: &'a str,
    pub weight: i64,
    pub top_class: &'a str,
    pub max_queued: Option<i64>,
    pub by: &'a str,
}

/// Is `label` a usable tenant label (also the hub-side display prefix)?
pub fn valid_label(label: &str) -> bool {
    (1..=64).contains(&label.len()) && label.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && label != LOCAL
}

/// Insert a server tenant (a fresh id). A revoked row for the same
/// fingerprint is replaced (its jobs keep their old tenant id).
pub fn insert_server_tenant(conn: &Connection, t: &NewTenant<'_>, at: i64) -> Result<TenantRow, String> {
    if !valid_label(t.label) {
        return Err("a tenant label is 1–64 of A-Z a-z 0-9 - _ . (and not 'local')".into());
    }
    if t.trust != "own" && t.trust != "team" {
        return Err("trust is own or team".into());
    }
    if class_rank(t.top_class).is_none() {
        return Err("top class is gate, interactive or batch".into());
    }
    if !(1..=100).contains(&t.weight) {
        return Err("weight is 1–100".into());
    }
    if let Some(old) = tenant_by_fp(conn, t.peer_fp) {
        if old.state != "revoked" {
            return Err(format!("that server is already tenant '{}' ({})", old.label, old.state));
        }
        // Free the fingerprint for the new row; the old row stays for its jobs.
        conn.execute("UPDATE compute_tenants SET peer_fp = NULL WHERE id = ?1", params![old.id]).map_err(|e| e.to_string())?;
    }
    let id = format!("tn-{}", &crypto::random_id()?[..16]);
    conn.execute(
        "INSERT INTO compute_tenants (id, kind, label, peer_fp, trust, weight, top_class, max_queued, state, epoch, created_by, created_at) \
         VALUES (?1, 'server', ?2, ?3, ?4, ?5, ?6, ?7, 'active', 1, ?8, ?9)",
        params![id, t.label, t.peer_fp, t.trust, t.weight, t.top_class, t.max_queued, t.by, at],
    )
    .map_err(|e| e.to_string())?;
    tenant_by_id(conn, &id).ok_or_else(|| "tenant vanished after insert".to_string())
}

/// Tenant settings the owner may change (None = leave as is).
#[derive(Debug, Clone, Default)]
pub struct TenantPatch {
    pub label: Option<String>,
    pub weight: Option<i64>,
    pub top_class: Option<String>,
    /// Some(None) clears the cap.
    pub max_running: Option<Option<i64>>,
    pub max_queued: Option<Option<i64>>,
    pub cpu_hours_day: Option<Option<i64>>,
    pub log_gb: Option<Option<i64>>,
    pub artifact_days: Option<Option<i64>>,
    pub blob_mb: Option<Option<i64>>,
}

pub fn update_tenant(conn: &Connection, id: &str, p: &TenantPatch) -> Result<TenantRow, String> {
    let Some(mut t) = tenant_by_id(conn, id) else { return Err("no such tenant".into()) };
    if let Some(l) = &p.label {
        if t.is_local() || !valid_label(l) {
            return Err("a tenant label is 1–64 of A-Z a-z 0-9 - _ . (and the local tenant keeps its name)".into());
        }
        t.label = l.clone();
    }
    if let Some(w) = p.weight {
        if !(1..=100).contains(&w) {
            return Err("weight is 1–100".into());
        }
        t.weight = w;
    }
    if let Some(c) = &p.top_class {
        if class_rank(c).is_none() {
            return Err("top class is gate, interactive or batch".into());
        }
        t.top_class = c.clone();
    }
    let pos = |v: Option<i64>, what: &str| -> Result<Option<i64>, String> {
        match v {
            Some(x) if x < 0 => Err(format!("{what} must be 0 or more")),
            other => Ok(other),
        }
    };
    if let Some(v) = p.max_running {
        t.max_running = pos(v, "max running")?;
    }
    if let Some(v) = p.max_queued {
        t.max_queued = pos(v, "max queued")?;
    }
    if let Some(v) = p.cpu_hours_day {
        t.cpu_hours_day = pos(v, "CPU-hours per day")?;
    }
    if let Some(v) = p.log_gb {
        t.log_gb = pos(v, "log GB")?;
    }
    if let Some(v) = p.artifact_days {
        t.artifact_days = pos(v, "artifact days")?;
    }
    if let Some(v) = p.blob_mb {
        t.blob_mb = pos(v, "blob MB")?;
    }
    conn.execute(
        "UPDATE compute_tenants SET label = ?2, weight = ?3, top_class = ?4, max_running = ?5, max_queued = ?6, \
         cpu_hours_day = ?7, log_gb = ?8, artifact_days = ?9, blob_mb = ?10 WHERE id = ?1",
        params![id, t.label, t.weight, t.top_class, t.max_running, t.max_queued, t.cpu_hours_day, t.log_gb, t.artifact_days, t.blob_mb],
    )
    .map_err(|e| e.to_string())?;
    tenant_by_id(conn, id).ok_or_else(|| "tenant vanished".into())
}

/// Suspend, resume or revoke. Revoke and suspend bump the epoch, so every
/// request signed before it is refused (V25). Returns the new row.
pub fn set_tenant_state(conn: &Connection, id: &str, state: &str) -> Result<TenantRow, String> {
    if id == LOCAL {
        return Err("the local tenant can't be suspended or revoked".into());
    }
    let Some(t) = tenant_by_id(conn, id) else { return Err("no such tenant".into()) };
    if t.state == "revoked" {
        return Err("that tenant is revoked; add the server again".into());
    }
    let bump = matches!(state, "suspended" | "revoked");
    match state {
        "active" | "suspended" | "revoked" => {}
        _ => return Err("state is active, suspended or revoked".into()),
    }
    conn.execute(
        "UPDATE compute_tenants SET state = ?2, epoch = epoch + ?3 WHERE id = ?1",
        params![id, state, bump as i64],
    )
    .map_err(|e| e.to_string())?;
    tenant_by_id(conn, id).ok_or_else(|| "tenant vanished".into())
}

// ── tenant grants ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TenantGrant {
    pub tenant_id: String,
    pub node_id: String,
    pub max_parallel: i64,
    pub max_job_secs: i64,
    pub max_disk_gb: i64,
    pub max_queued: i64,
    pub granted_by: String,
    pub granted_at: i64,
}

fn tgrant_from_row(r: &Row<'_>) -> rusqlite::Result<TenantGrant> {
    Ok(TenantGrant {
        tenant_id: r.get(0)?,
        node_id: r.get(1)?,
        max_parallel: r.get(2)?,
        max_job_secs: r.get(3)?,
        max_disk_gb: r.get(4)?,
        max_queued: r.get(5)?,
        granted_by: r.get(6)?,
        granted_at: r.get(7)?,
    })
}

const TGRANT_COLS: &str = "tenant_id, node_id, max_parallel, max_job_secs, max_disk_gb, max_queued, granted_by, granted_at";

pub fn tenant_grant(conn: &Connection, tenant_id: &str, node_id: &str) -> Option<TenantGrant> {
    conn.query_row(
        &format!("SELECT {TGRANT_COLS} FROM compute_tenant_grants WHERE tenant_id = ?1 AND node_id = ?2"),
        params![tenant_id, node_id],
        tgrant_from_row,
    )
    .optional()
    .ok()
    .flatten()
}

pub fn tenant_grants(conn: &Connection, tenant_id: Option<&str>, node_id: Option<&str>) -> Vec<TenantGrant> {
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {TGRANT_COLS} FROM compute_tenant_grants WHERE (?1 IS NULL OR tenant_id = ?1) AND (?2 IS NULL OR node_id = ?2) \
         ORDER BY tenant_id, node_id"
    )) else {
        return Vec::new();
    };
    stmt.query_map(params![tenant_id, node_id], tgrant_from_row).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

pub fn set_tenant_grant(conn: &Connection, tenant_id: &str, node_id: &str, l: &super::store::GrantLimits, by: &str, at: i64) -> Result<TenantGrant, String> {
    conn.execute(
        "INSERT INTO compute_tenant_grants (tenant_id, node_id, max_parallel, max_job_secs, max_disk_gb, max_queued, granted_by, granted_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) ON CONFLICT(tenant_id, node_id) DO UPDATE SET max_parallel = excluded.max_parallel, \
         max_job_secs = excluded.max_job_secs, max_disk_gb = excluded.max_disk_gb, max_queued = excluded.max_queued, \
         granted_by = excluded.granted_by, granted_at = excluded.granted_at",
        params![
            tenant_id,
            node_id,
            l.max_parallel.unwrap_or(super::DEFAULT_MAX_PARALLEL),
            l.max_job_secs.unwrap_or(super::DEFAULT_MAX_JOB_SECS),
            l.max_disk_gb.unwrap_or(super::DEFAULT_MAX_DISK_GB),
            l.max_queued.unwrap_or(super::DEFAULT_MAX_QUEUED),
            by,
            at
        ],
    )
    .map_err(|e| e.to_string())?;
    tenant_grant(conn, tenant_id, node_id).ok_or_else(|| "grant vanished after write".into())
}

pub fn revoke_tenant_grant(conn: &Connection, tenant_id: &str, node_id: &str) -> Result<bool, String> {
    conn.execute("DELETE FROM compute_tenant_grants WHERE tenant_id = ?1 AND node_id = ?2", params![tenant_id, node_id])
        .map(|n| n > 0)
        .map_err(|e| e.to_string())
}

// ── job keys ─────────────────────────────────────────────────────────

/// A remote workspace id a tenant may name: 1–128 of A-Z a-z 0-9 - _ .
pub fn valid_remote_id(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
}

/// The hub-side workspace key of a tenant job: `t/<tenant>/<remote ws>`.
pub fn tenant_ws_key(tenant_id: &str, remote_ws: &str) -> String {
    format!("t/{tenant_id}/{remote_ws}")
}

/// `(tenant, remote ws)` of a [`tenant_ws_key`]; None for a local project id.
pub fn parse_tenant_ws_key(key: &str) -> Option<(&str, &str)> {
    let rest = key.strip_prefix("t/")?;
    let (t, w) = rest.split_once('/')?;
    (!t.is_empty() && !w.is_empty()).then_some((t, w))
}

/// Names a tenant job's mirror and warm slots on a node (V28): derived from
/// the hub's tenant id, the tenant's workspace id and the remote, under its
/// own domain, so two tenants that name the same workspace id and remote
/// never share a mirror or a warm `target/`, and a tenant can never land on
/// a local workspace's key (that one is `sync::project_key`, another domain).
pub fn hub_project_key(tenant_id: &str, remote_ws: &str, remote: Option<&str>) -> String {
    let h = crypto::sha256_hex(&crypto::signed_bytes(
        "k2-compute-project-tenant-v1",
        &[tenant_id.as_bytes(), remote_ws.as_bytes(), remote.unwrap_or("").as_bytes()],
    ));
    h[..16].to_string()
}

/// `~/.k2/compute/tenants/<id>/` (logs, artifacts, blobs, mirrors).
pub fn tenant_dir(tenant_id: &str) -> std::path::PathBuf {
    super::compute_dir().join("tenants").join(tenant_id)
}

/// A tenant's bare git mirror for one project key (S2).
pub fn tenant_mirror(tenant_id: &str, project_key: &str) -> std::path::PathBuf {
    tenant_dir(tenant_id).join("repos").join(format!("{project_key}.git"))
}

/// A tenant's stored blob.
pub fn blob_path(tenant_id: &str, sha256: &str) -> std::path::PathBuf {
    tenant_dir(tenant_id).join("blobs").join(sha256)
}

pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

// ── blob index ───────────────────────────────────────────────────────

/// Bytes a tenant's blob store holds.
pub fn blob_bytes(conn: &Connection, tenant_id: &str) -> u64 {
    conn.query_row("SELECT COALESCE(SUM(bytes), 0) FROM compute_blobs WHERE tenant_id = ?1", params![tenant_id], |r| r.get::<_, i64>(0))
        .unwrap_or(0)
        .max(0) as u64
}

pub fn blob_known(conn: &Connection, tenant_id: &str, sha256: &str) -> Option<(String, u64)> {
    conn.query_row(
        "SELECT kind, bytes FROM compute_blobs WHERE tenant_id = ?1 AND sha256 = ?2",
        params![tenant_id, sha256],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64)),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn record_blob(conn: &Connection, tenant_id: &str, sha256: &str, kind: &str, bytes: u64, at: i64) -> Result<(), String> {
    conn.execute(
        "INSERT INTO compute_blobs (tenant_id, sha256, kind, bytes, created_at, last_used) VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
         ON CONFLICT(tenant_id, sha256) DO UPDATE SET last_used = excluded.last_used",
        params![tenant_id, sha256, kind, bytes as i64, at],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

pub fn touch_blob(conn: &Connection, tenant_id: &str, sha256: &str, at: i64) {
    let _ = conn.execute("UPDATE compute_blobs SET last_used = ?3 WHERE tenant_id = ?1 AND sha256 = ?2", params![tenant_id, sha256, at]);
}

/// Blobs not used since `before`: removed from the index; the caller
/// deletes the files. Returns `(tenant, sha)` pairs.
pub fn expire_blobs(conn: &Connection, before: i64) -> Vec<(String, String)> {
    let old: Vec<(String, String)> = conn
        .prepare("SELECT tenant_id, sha256 FROM compute_blobs WHERE last_used < ?1")
        .and_then(|mut s| s.query_map(params![before], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.filter_map(Result::ok).collect()))
        .unwrap_or_default();
    let _ = conn.execute("DELETE FROM compute_blobs WHERE last_used < ?1", params![before]);
    old
}

// ── signed requests (V9) ─────────────────────────────────────────────

/// The parts of an `X-K2-Hub` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubAuth {
    pub fp: String,
    pub ts: i64,
    pub nonce: String,
    pub epoch: i64,
    pub sig: String,
}

impl HubAuth {
    /// `fp=…, ts=…, nonce=…, epoch=…, sig=…` (order-free on parse).
    pub fn to_header(&self) -> String {
        format!("fp={}, ts={}, nonce={}, epoch={}, sig={}", self.fp, self.ts, self.nonce, self.epoch, self.sig)
    }

    pub fn parse(v: &str) -> Option<HubAuth> {
        let mut m: HashMap<&str, &str> = HashMap::new();
        for part in v.split(',') {
            let (k, x) = part.trim().split_once('=')?;
            if m.insert(k.trim(), x.trim()).is_some() {
                return None;
            }
        }
        let fp = m.get("fp")?.to_string();
        let nonce = m.get("nonce")?.to_string();
        let sig = m.get("sig")?.to_string();
        if !is_sha256_hex(&fp) || !(16..=64).contains(&nonce.len()) || !nonce.chars().all(|c| c.is_ascii_hexdigit()) || sig.is_empty() {
            return None;
        }
        Some(HubAuth { fp, ts: m.get("ts")?.parse().ok()?, nonce, epoch: m.get("epoch")?.parse().ok()?, sig })
    }
}

/// The signed parts, in order.
#[allow(clippy::too_many_arguments)]
fn parts<'a>(method: &'a str, path: &'a str, query: &'a str, body_sha: &'a str, fp: &'a str, ts: &'a str, nonce: &'a str, epoch: &'a str) -> [&'a [u8]; 8] {
    [method.as_bytes(), path.as_bytes(), query.as_bytes(), body_sha.as_bytes(), fp.as_bytes(), ts.as_bytes(), nonce.as_bytes(), epoch.as_bytes()]
}

/// Sign a hub request. `body_sha` = sha256 hex of the exact body bytes
/// (for the blob route: the blob's hash, which is also its `sha256` param).
pub fn sign_request(key: &crypto::SigningKey, method: &str, path: &str, query: &str, body_sha: &str, epoch: i64, ts: i64) -> Result<HubAuth, String> {
    let fp = key.fingerprint();
    let nonce = crypto::hex(&crypto::random_bytes(16)?);
    let (ts_s, ep_s) = (ts.to_string(), epoch.to_string());
    let sig = key.sign_domain(DOMAIN, &parts(method, path, query, body_sha, &fp, &ts_s, &nonce, &ep_s))?;
    Ok(HubAuth { fp, ts, nonce, epoch, sig })
}

/// Does `a.sig` verify against `spki_der` for this request?
pub fn signature_ok(a: &HubAuth, spki_der: &[u8], method: &str, path: &str, query: &str, body_sha: &str) -> bool {
    let (ts_s, ep_s) = (a.ts.to_string(), a.epoch.to_string());
    crypto::verify_domain(spki_der, DOMAIN, &parts(method, path, query, body_sha, &a.fp, &ts_s, &a.nonce, &ep_s), &a.sig)
}

/// Why a signed request was refused: `(code, http status, hint)`.
pub type AuthRefusal = (&'static str, u16, String);

/// What the hub knows about the caller's peer (from the federation store).
#[derive(Debug, Clone)]
pub struct PeerFacts {
    pub spki_der: Vec<u8>,
    pub trusted: bool,
    pub has_compute: bool,
    pub pairing_version: u32,
}

/// Verify a tenant request, fail-closed, in this order: skew → pinned key
/// (signature) → peer gate (Trusted + `compute`) → pairing v2 → tenant row
/// → tenant state → epoch → nonce. `hello` may carry any epoch (it is how a
/// caller learns the current one); every other request must carry the
/// current epoch. On success the nonce is remembered (a replay of the
/// same request is refused) and the tenant row is returned.
#[allow(clippy::too_many_arguments)]
pub fn verify_request(
    a: &HubAuth,
    peer: Option<&PeerFacts>,
    tenant: Option<&TenantRow>,
    method: &str,
    path: &str,
    query: &str,
    body_sha: &str,
    now: i64,
    hello: bool,
) -> Result<TenantRow, AuthRefusal> {
    if (now - a.ts).abs() > SKEW_SECS {
        return Err((codes::SKEW, 401, format!("the request's clock is {}s off this hub's (at most {SKEW_SECS}s)", now - a.ts)));
    }
    let Some(peer) = peer else {
        return Err((codes::TENANT_UNKNOWN, 403, "this hub doesn't know your server: pair the two servers (federation, pairing v2), then the hub owner adds you as a tenant".into()));
    };
    if !signature_ok(a, &peer.spki_der, method, path, query, body_sha) {
        return Err((codes::BAD_SIGNATURE, 401, "the request signature doesn't verify against your server's pinned key".into()));
    }
    if !peer.trusted || !peer.has_compute {
        return Err((codes::PEER_NOT_GRANTED, 403, "your server isn't granted compute on this hub (the hub owner: k2 compute tenants add)".into()));
    }
    if peer.pairing_version < 2 {
        return Err((codes::PAIRING_V2_REQUIRED, 403, "compute needs the newer pairing (v2): pair the two servers again".into()));
    }
    let Some(t) = tenant.filter(|t| t.peer_fp.as_deref() == Some(a.fp.as_str())) else {
        return Err((codes::TENANT_UNKNOWN, 403, "your server isn't a tenant of this hub (the hub owner: k2 compute tenants add)".into()));
    };
    match t.state.as_str() {
        "active" => {}
        "suspended" => return Err((codes::TENANT_SUSPENDED, 403, "the hub owner suspended your server's compute".into())),
        _ => return Err((codes::TENANT_REVOKED, 403, "the hub owner revoked your server's compute".into())),
    }
    if !hello && a.epoch != t.epoch {
        return Err((codes::EPOCH_STALE, 409, format!("tenant epoch {} is stale (now {}): say hello again", a.epoch, t.epoch)));
    }
    if !remember_nonce(&t.id, &a.nonce, a.ts, now) {
        return Err((codes::REPLAY, 401, "this request was already used (replayed nonce)".into()));
    }
    Ok(t.clone())
}

/// Per-tenant nonce → expiry. In memory: after a restart a replay is still
/// bounded by the skew window (the roster/ingress rule).
static NONCES: Mutex<Option<HashMap<String, HashMap<String, i64>>>> = Mutex::new(None);

/// Record a nonce; false when it was already seen and hasn't expired.
pub fn remember_nonce(tenant_id: &str, nonce: &str, ts: i64, now: i64) -> bool {
    let mut g = NONCES.lock().unwrap_or_else(|p| p.into_inner());
    let per = g.get_or_insert_with(HashMap::new).entry(tenant_id.to_string()).or_default();
    if per.len() >= NONCE_CAP / 2 {
        per.retain(|_, exp| *exp >= now);
    }
    if per.get(nonce).is_some_and(|exp| *exp >= now) {
        return false;
    }
    if per.len() >= NONCE_CAP {
        // Full of live nonces: drop the ones closest to expiry.
        let mut by_exp: Vec<(String, i64)> = per.iter().map(|(k, v)| (k.clone(), *v)).collect();
        by_exp.sort_by_key(|(_, e)| *e);
        for (k, _) in by_exp.into_iter().take(NONCE_CAP / 4) {
            per.remove(&k);
        }
    }
    per.insert(nonce.to_string(), ts.max(now) + SKEW_SECS);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(fp: &str) -> TenantRow {
        TenantRow {
            id: "tn-a".into(),
            kind: "server".into(),
            label: "sewlab".into(),
            peer_fp: Some(fp.into()),
            trust: "own".into(),
            weight: 1,
            top_class: "interactive".into(),
            max_running: None,
            max_queued: None,
            cpu_hours_day: None,
            log_gb: None,
            artifact_days: None,
            blob_mb: None,
            state: "active".into(),
            epoch: 3,
            created_by: None,
            created_at: None,
        }
    }

    fn peer(k: &crypto::SigningKey) -> PeerFacts {
        PeerFacts { spki_der: k.spki_der().to_vec(), trusted: true, has_compute: true, pairing_version: 2 }
    }

    const BODY: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn a_good_request_verifies_once_and_a_replay_is_refused() {
        let k = crypto::SigningKey::generate().unwrap();
        let now = 1_000_000;
        let a = sign_request(&k, "POST", "/cli/compute/hub/run", "", BODY, 3, now).unwrap();
        let parsed = HubAuth::parse(&a.to_header()).expect("header round trip");
        assert_eq!(parsed, a);
        let mut t = tenant(&k.fingerprint());
        t.id = format!("tn-replay-{}", now);
        let ok = verify_request(&parsed, Some(&peer(&k)), Some(&t), "POST", "/cli/compute/hub/run", "", BODY, now, false);
        assert_eq!(ok.expect("verifies").id, t.id);
        let again = verify_request(&parsed, Some(&peer(&k)), Some(&t), "POST", "/cli/compute/hub/run", "", BODY, now + 1, false);
        assert_eq!(again.unwrap_err().0, codes::REPLAY);
    }

    #[test]
    fn every_refusal_has_its_own_code() {
        let k = crypto::SigningKey::generate().unwrap();
        let other = crypto::SigningKey::generate().unwrap();
        let now = 2_000_000;
        let t = tenant(&k.fingerprint());
        let p = peer(&k);
        let sign = |epoch: i64, ts: i64| sign_request(&k, "POST", "/cli/compute/hub/jobs", "", BODY, epoch, ts).unwrap();
        let check = |a: &HubAuth, p: Option<&PeerFacts>, t: Option<&TenantRow>| {
            verify_request(a, p, t, "POST", "/cli/compute/hub/jobs", "", BODY, now, false).unwrap_err().0
        };
        // Skew (either direction).
        assert_eq!(check(&sign(3, now - SKEW_SECS - 1), Some(&p), Some(&t)), codes::SKEW);
        assert_eq!(check(&sign(3, now + SKEW_SECS + 1), Some(&p), Some(&t)), codes::SKEW);
        // Unknown peer.
        assert_eq!(check(&sign(3, now), None, Some(&t)), codes::TENANT_UNKNOWN);
        // Bad signature: another key, a tampered body hash, another path.
        let forged = sign_request(&other, "POST", "/cli/compute/hub/jobs", "", BODY, 3, now).unwrap();
        let forged = HubAuth { fp: k.fingerprint(), ..forged };
        assert_eq!(check(&forged, Some(&p), Some(&t)), codes::BAD_SIGNATURE);
        let a = sign(3, now);
        let tampered = verify_request(&a, Some(&p), Some(&t), "POST", "/cli/compute/hub/jobs", "", &"1".repeat(64), now, false);
        assert_eq!(tampered.unwrap_err().0, codes::BAD_SIGNATURE);
        let moved = verify_request(&a, Some(&p), Some(&t), "POST", "/cli/compute/hub/jobs/cancel", "", BODY, now, false);
        assert_eq!(moved.unwrap_err().0, codes::BAD_SIGNATURE);
        // Peer not granted compute / not trusted.
        let no_cap = PeerFacts { has_compute: false, ..p.clone() };
        assert_eq!(check(&sign(3, now), Some(&no_cap), Some(&t)), codes::PEER_NOT_GRANTED);
        let pending = PeerFacts { trusted: false, ..p.clone() };
        assert_eq!(check(&sign(3, now), Some(&pending), Some(&t)), codes::PEER_NOT_GRANTED);
        // A v1-paired peer.
        let v1 = PeerFacts { pairing_version: 1, ..p.clone() };
        assert_eq!(check(&sign(3, now), Some(&v1), Some(&t)), codes::PAIRING_V2_REQUIRED);
        // No tenant row, or a row bound to another fingerprint.
        assert_eq!(check(&sign(3, now), Some(&p), None), codes::TENANT_UNKNOWN);
        let elsewhere = TenantRow { peer_fp: Some(other.fingerprint()), ..t.clone() };
        assert_eq!(check(&sign(3, now), Some(&p), Some(&elsewhere)), codes::TENANT_UNKNOWN);
        // Suspended, revoked.
        let sus = TenantRow { state: "suspended".into(), ..t.clone() };
        assert_eq!(check(&sign(3, now), Some(&p), Some(&sus)), codes::TENANT_SUSPENDED);
        let rev = TenantRow { state: "revoked".into(), ..t.clone() };
        assert_eq!(check(&sign(3, now), Some(&p), Some(&rev)), codes::TENANT_REVOKED);
        // A bumped epoch kills requests signed under the old one; hello
        // (how a caller learns it) takes any epoch.
        assert_eq!(check(&sign(2, now), Some(&p), Some(&t)), codes::EPOCH_STALE);
        assert_eq!(check(&sign(0, now), Some(&p), Some(&t)), codes::EPOCH_STALE);
        let hello = sign_request(&k, "POST", "/cli/compute/hub/hello", "", BODY, 0, now).unwrap();
        let mut ht = t.clone();
        ht.id = "tn-hello-epoch".into();
        assert!(verify_request(&hello, Some(&p), Some(&ht), "POST", "/cli/compute/hub/hello", "", BODY, now, true).is_ok());
    }

    #[test]
    fn the_header_parser_refuses_junk() {
        assert!(HubAuth::parse("").is_none());
        assert!(HubAuth::parse("fp=abc, ts=1, nonce=00112233445566778899, epoch=1, sig=x").is_none(), "fp must be a sha256");
        let fp = "a".repeat(64);
        assert!(HubAuth::parse(&format!("fp={fp}, ts=x, nonce=00112233445566778899, epoch=1, sig=x")).is_none());
        assert!(HubAuth::parse(&format!("fp={fp}, ts=1, nonce=short, epoch=1, sig=x")).is_none());
        assert!(HubAuth::parse(&format!("fp={fp}, fp={fp}, ts=1, nonce=00112233445566778899, epoch=1, sig=x")).is_none(), "duplicate key");
        assert!(HubAuth::parse(&format!("fp={fp}, ts=1, nonce=00112233445566778899, epoch=1, sig=x")).is_some());
    }

    #[test]
    fn project_keys_never_cross_tenants_or_the_local_namespace() {
        let remote = Some("https://github.com/acme/app");
        let a = hub_project_key("tn-a", "ws-1", remote);
        let b = hub_project_key("tn-b", "ws-1", remote);
        assert_ne!(a, b, "two tenants naming the same workspace id + remote get different mirrors");
        assert_eq!(a, hub_project_key("tn-a", "ws-1", remote), "stable for one tenant");
        assert_ne!(a, hub_project_key("tn-a", "ws-2", remote));
        assert_ne!(a, hub_project_key("tn-a", "ws-1", None));
        // A tenant claiming a local project's id never lands on its key.
        assert_ne!(hub_project_key("tn-a", "ws-1", remote), super::super::sync::project_key("ws-1", remote));
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn tenant_ws_keys_round_trip() {
        let k = tenant_ws_key("tn-a", "ws-1");
        assert_eq!(k, "t/tn-a/ws-1");
        assert_eq!(parse_tenant_ws_key(&k), Some(("tn-a", "ws-1")));
        assert_eq!(parse_tenant_ws_key("0b8f5a3e-1111-2222-3333-444455556666"), None);
        assert!(!valid_remote_id("a/b"), "a slash would forge another key");
        assert!(!valid_remote_id(""));
        assert!(valid_remote_id("0b8f5a3e-1111-2222-3333-444455556666"));
    }

    #[test]
    fn classes_follow_the_tenants_top_class() {
        let mut t = tenant(&"a".repeat(64));
        assert!(t.allows_class("interactive"));
        assert!(t.allows_class("batch"));
        assert!(!t.allows_class("gate"));
        assert!(!t.allows_class("urgent"));
        t.top_class = "gate".into();
        assert!(t.allows_class("gate"));
    }

    #[test]
    fn tenants_store_round_trip_on_a_migrated_db() {
        let conn = crate::db::isolated_test_connection();
        let local = tenant_by_id(&conn, LOCAL).expect("the migration seeds the local tenant");
        assert_eq!((local.kind.as_str(), local.weight, local.state.as_str()), ("local", 1, "active"));
        let fp = "b".repeat(64);
        let t = insert_server_tenant(
            &conn,
            &NewTenant { label: "rosson.k2.dev", peer_fp: &fp, trust: "own", weight: 2, top_class: "gate", max_queued: None, by: "owner" },
            10,
        )
        .unwrap();
        assert_eq!((t.weight, t.top_class.as_str(), t.epoch), (2, "gate", 1));
        assert!(insert_server_tenant(&conn, &NewTenant { label: "again", peer_fp: &fp, trust: "own", weight: 1, top_class: "batch", max_queued: None, by: "o" }, 11).is_err(), "one live tenant per server");
        assert!(insert_server_tenant(&conn, &NewTenant { label: "local", peer_fp: &"c".repeat(64), trust: "own", weight: 1, top_class: "batch", max_queued: None, by: "o" }, 11).is_err());
        let s = set_tenant_state(&conn, &t.id, "suspended").unwrap();
        assert_eq!((s.state.as_str(), s.epoch), ("suspended", 2), "suspend bumps the epoch");
        let r = set_tenant_state(&conn, &t.id, "active").unwrap();
        assert_eq!(r.epoch, 2, "resume keeps it");
        let r = set_tenant_state(&conn, &t.id, "revoked").unwrap();
        assert_eq!((r.state.as_str(), r.epoch), ("revoked", 3));
        assert!(set_tenant_state(&conn, LOCAL, "revoked").is_err());
        // Re-adding a revoked server makes a new row.
        let t2 = insert_server_tenant(&conn, &NewTenant { label: "rosson2", peer_fp: &fp, trust: "own", weight: 1, top_class: "interactive", max_queued: Some(5), by: "o" }, 12).unwrap();
        assert_ne!(t2.id, t.id);
        assert_eq!(tenant_by_fp(&conn, &fp).unwrap().id, t2.id);
        assert_eq!(find_tenant(&conn, "rosson2").unwrap().id, t2.id);
        let p = update_tenant(&conn, &t2.id, &TenantPatch { weight: Some(3), max_queued: Some(None), ..Default::default() }).unwrap();
        assert_eq!((p.weight, p.max_queued), (3, None));
        assert!(update_tenant(&conn, &t2.id, &TenantPatch { weight: Some(0), ..Default::default() }).is_err());
    }
}
