//! The compute hub, submitter side (`prd-compute-shared-queue-v1` S2,
//! V20/V21/V24; 0.46.3). "Use compute hub <host>" turns this server into a
//! tenant of another K2 server that owns the nodes and the queue.
//!
//! - **Config** (owner, `/cli/compute/hubs*`, `k2 compute hub use|off|
//!   status`): the hub is a paired federation peer (Trusted, pairing v2)
//!   whose owner added this server as a tenant (`hello` must answer).
//!   Migration 0206: `compute_hubs` (one `active` row).
//! - **Proxy** ([`intercept`]): with a hub active, `k2 compute run` (and
//!   `jobs`, an agent's `nodes`) go to the hub; a job id this server sent
//!   to a hub (`compute_remote_jobs`) is read, followed, fetched, cancelled
//!   and retried there, even after the hub is switched off. Local checks
//!   stay local: the passport, the workspace switch "Allow compute nodes",
//!   the caller's own workspace and session. The hub only ever sees this
//!   server's ids as remote ids, signed (`X-K2-Hub`, federation key).
//! - **Code** (V20): the commit is resolved here as today (`source_for`);
//!   `prepare` asks which recent commits the hub's mirror already has, the
//!   `have..commit` bundle and any `--dirty` blob go up the streaming blob
//!   route (file-backed, never read whole), then `run` names the commit.
//! - **Completion** (V24): one poller (`changes` long-poll per hub) updates
//!   `compute_remote_jobs` and delivers the `[compute]` message locally to
//!   the session that asked, once (`notified_at`). No federation message.
//! - **Hub down**: fail fast, `hub_unreachable` (503 → the CLI's exit 3)
//!   with a hint. No automatic fallback to this server's own nodes (Q8).
//!
//! With no active hub and no remote job ids, nothing here changes how
//! this server's own controller behaves.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, Once};
use std::time::Duration;

use k2_core::compute::hub::{self as hub, codes as hcodes};
use k2_core::compute::proto::crypto::{self, SigningKey};
use k2_core::compute::proto::frames::JobState;
use k2_core::compute::{self as compute, codes};
use k2_core::federation::PeerStore;
use rusqlite::{params, Connection, OptionalExtension};

use crate::compute_routes::{self as cr, b, json_body, n, no, s, Refusal, Who, R};
use crate::compute_ws;

/// The submitter's own hub config routes (local callers).
pub const ROUTES: &[&str] = &["/cli/compute/hubs", "/cli/compute/hubs/off", "/cli/compute/hubs/use"];
pub const POST_ROUTES: &[&str] = &["/cli/compute/hubs/off", "/cli/compute/hubs/use"];

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

// ── rows (0206) ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubRow {
    pub fp: String,
    pub label: String,
    pub base_url: String,
    pub state: String,
    pub since: i64,
    pub set_by: String,
}

fn hub_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<HubRow> {
    Ok(HubRow { fp: r.get(0)?, label: r.get(1)?, base_url: r.get(2)?, state: r.get(3)?, since: r.get(4)?, set_by: r.get(5)? })
}

const HUB_COLS: &str = "hub_fp, label, base_url, state, since, set_by";

pub fn active_hub(conn: &Connection) -> Option<HubRow> {
    conn.query_row(&format!("SELECT {HUB_COLS} FROM compute_hubs WHERE state = 'active'"), [], hub_from).optional().ok().flatten()
}

fn hub_by_fp(conn: &Connection, fp: &str) -> Option<HubRow> {
    conn.query_row(&format!("SELECT {HUB_COLS} FROM compute_hubs WHERE hub_fp = ?1"), params![fp], hub_from).optional().ok().flatten()
}

fn set_active(conn: &Connection, h: &HubRow) -> Result<(), String> {
    conn.execute("UPDATE compute_hubs SET state = 'off' WHERE state = 'active'", []).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO compute_hubs (hub_fp, label, base_url, state, since, set_by) VALUES (?1, ?2, ?3, 'active', ?4, ?5) \
         ON CONFLICT(hub_fp) DO UPDATE SET label = excluded.label, base_url = excluded.base_url, state = 'active', \
         since = excluded.since, set_by = excluded.set_by",
        params![h.fp, h.label, h.base_url, h.since, h.set_by],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// One job this server sent to a hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteJob {
    pub hub_job_id: String,
    pub hub_fp: String,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub client_job_id: String,
    pub detach: bool,
    pub state: String,
    pub node: Option<String>,
    pub created_at: i64,
    pub notified_at: Option<i64>,
}

const RJ_COLS: &str = "hub_job_id, hub_fp, workspace_id, session_id, client_job_id, detach, state, node, created_at, notified_at";

fn rj_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<RemoteJob> {
    Ok(RemoteJob {
        hub_job_id: r.get(0)?,
        hub_fp: r.get(1)?,
        workspace_id: r.get(2)?,
        session_id: r.get(3)?,
        client_job_id: r.get(4)?,
        detach: r.get::<_, i64>(5)? != 0,
        state: r.get(6)?,
        node: r.get(7)?,
        created_at: r.get(8)?,
        notified_at: r.get(9)?,
    })
}

pub fn remote_job(conn: &Connection, id: &str) -> Option<RemoteJob> {
    conn.query_row(&format!("SELECT {RJ_COLS} FROM compute_remote_jobs WHERE hub_job_id = ?1"), params![id], rj_from).optional().ok().flatten()
}

/// A remote job by id or unique hex prefix (≥ 4), as the CLI's short ids.
pub fn remote_job_by_token(conn: &Connection, token: &str) -> Option<RemoteJob> {
    let t = token.trim();
    if let Some(j) = remote_job(conn, t) {
        return Some(j);
    }
    if t.len() < 4 || !t.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut st = conn.prepare(&format!("SELECT {RJ_COLS} FROM compute_remote_jobs WHERE hub_job_id LIKE ?1 || '%' LIMIT 2")).ok()?;
    let rows: Vec<RemoteJob> = st.query_map(params![t], rj_from).ok()?.filter_map(Result::ok).collect();
    (rows.len() == 1).then(|| rows.into_iter().next()).flatten()
}

fn is_terminal(state: &str) -> bool {
    JobState::parse(state).is_some_and(JobState::is_terminal)
}

/// Jobs still running somewhere (the poller's list).
fn open_remote_jobs(conn: &Connection) -> Vec<RemoteJob> {
    let Ok(mut st) = conn.prepare(&format!(
        "SELECT {RJ_COLS} FROM compute_remote_jobs WHERE state NOT IN ('done','failed','cancelled','timeout','interrupted','unknown') \
         ORDER BY created_at LIMIT 500"
    )) else {
        return Vec::new();
    };
    st.query_map([], rj_from).map(|rows| rows.filter_map(Result::ok).collect()).unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn record_remote(conn: &Connection, id: &str, hub_fp: &str, ws: &str, session: Option<&str>, client: &str, detach: bool, state: &str, node: Option<&str>) {
    let now = compute::now();
    let _ = conn.execute(
        "INSERT INTO compute_remote_jobs (hub_job_id, hub_fp, workspace_id, session_id, client_job_id, detach, state, node, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9) ON CONFLICT(hub_job_id) DO NOTHING",
        params![id, hub_fp, ws, session, client, detach as i64, state, node, now],
    );
}

fn update_remote(conn: &Connection, id: &str, state: &str, node: Option<&str>, summary: Option<&str>, ended_at: Option<i64>) {
    let _ = conn.execute(
        "UPDATE compute_remote_jobs SET state = ?2, node = COALESCE(?3, node), summary_json = COALESCE(?4, summary_json), \
         ended_at = COALESCE(?5, ended_at), updated_at = ?6 WHERE hub_job_id = ?1",
        params![id, state, node, summary, ended_at, compute::now()],
    );
}

/// Compare-and-set: true only for the one caller that marks it.
fn mark_notified(conn: &Connection, id: &str) -> bool {
    conn.execute("UPDATE compute_remote_jobs SET notified_at = ?2 WHERE hub_job_id = ?1 AND notified_at IS NULL", params![id, compute::now()])
        .is_ok_and(|n| n == 1)
}

// ── the signed client ────────────────────────────────────────────────

/// Epoch last heard per hub (`hello`).
static EPOCHS: Mutex<Option<HashMap<String, i64>>> = Mutex::new(None);

fn identity_key() -> R<SigningKey> {
    let kp = k2_core::federation::identity::load_or_create().map_err(|e| no("internal", 500, format!("federation identity: {e}")))?;
    SigningKey::from_pkcs8_der(&kp.serialize_der()).map_err(|e| no("internal", 500, e))
}

/// Where to dial the hub: the peer's address now (it may have moved, a
/// signed address notice), else what was stored.
fn dial_base(h: &HubRow) -> R<String> {
    if let Ok(st) = PeerStore::load() {
        if let Some(p) = st.get(&h.fp) {
            if let Ok(b) = crate::federation_routes::gated_peer_base_url(p) {
                if !b.is_empty() {
                    return Ok(b);
                }
            }
        }
    }
    if h.base_url.is_empty() {
        return Err(unreachable(h, "no dial address for it"));
    }
    Ok(h.base_url.clone())
}

fn unreachable(h: &HubRow, why: &str) -> Refusal {
    no(
        hcodes::HUB_UNREACHABLE,
        503,
        format!("the compute hub {} is unreachable ({why}); jobs already there keep running. No automatic fallback: try again, or `k2 compute hub off` to use this server's own nodes", h.label),
    )
}

/// The hub's refusal codes as the `&'static str` a [`Refusal`] carries.
fn static_code(c: &str) -> &'static str {
    const KNOWN: &[&str] = &[
        codes::COMPUTE_OFF,
        codes::NOT_GRANTED,
        codes::NODE_OFFLINE,
        codes::NODE_PAUSED,
        codes::NODE_NOT_FOUND,
        codes::QUEUE_FULL,
        codes::OWNER_ONLY,
        codes::IDEMPOTENCY_CONFLICT,
        codes::JOB_NOT_FOUND,
        codes::BAD_REQUEST,
        codes::NOT_A_REPO,
        codes::DIRTY_TOO_LARGE,
        codes::NO_NODE_FITS,
        hcodes::AUTH_MISSING,
        hcodes::SKEW,
        hcodes::BAD_SIGNATURE,
        hcodes::REPLAY,
        hcodes::TENANT_UNKNOWN,
        hcodes::PEER_NOT_GRANTED,
        hcodes::PAIRING_V2_REQUIRED,
        hcodes::TENANT_SUSPENDED,
        hcodes::TENANT_REVOKED,
        hcodes::EPOCH_STALE,
        hcodes::CLASS_NOT_ALLOWED,
        hcodes::BLOB_TOO_LARGE,
        hcodes::BLOB_QUOTA,
        hcodes::BLOB_MISMATCH,
        hcodes::SOURCE_MISSING,
        "already_ended",
        "job_started",
        "still_running",
        "nothing_to_retry",
        "no_receipt",
        "artifact_not_found",
        "artifact_gone",
        "artifact_timeout",
        "artifact_unavailable",
        "artifact_changed",
        "relay_cap",
        "node_upgrade_required",
        "need_invalid",
        "tool_unsupported",
        "env_refused",
        "timeout_over_grant",
        "exceeds_node_caps",
        "workspace_not_found",
        "bundle_prereq_missing",
        "bundle_invalid",
        "compute_disabled",
        "federation_off",
        "forbidden",
        "not_found",
    ];
    KNOWN.iter().copied().find(|k| *k == c).unwrap_or("hub_refused")
}

fn refusal_from(status: u16, v: &serde_json::Value) -> Refusal {
    let code = v["error"]["code"].as_str().unwrap_or("hub_refused");
    let hint = v["error"]["hint"].as_str().unwrap_or("the hub refused the request").to_string();
    let c = static_code(code);
    let hint = if c == "hub_refused" { format!("{code}: {hint}") } else { hint };
    no(c, if status == 0 { 502 } else { status }, format!("hub: {hint}"))
}

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        // Never follow a redirect: a hub must not steer us elsewhere.
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("http client: {e}"))
}

/// POST signed bytes; `(status, JSON body)` or a transport error.
#[allow(clippy::too_many_arguments)]
fn post_signed(base: &str, key: &SigningKey, path: &str, query: &str, body: reqwest::blocking::Body, body_sha: &str, epoch: i64, timeout: Duration) -> Result<(u16, serde_json::Value), String> {
    let a = hub::sign_request(key, "POST", path, query, body_sha, epoch, compute::now())?;
    let url = if query.is_empty() { format!("{base}{path}") } else { format!("{base}{path}?{query}") };
    let resp = http()?
        .post(&url)
        .timeout(timeout)
        .header(hub::HEADER, a.to_header())
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .map_err(|e| e.to_string())?;
    let status = resp.status().as_u16();
    let text = resp.text().map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|_| serde_json::json!({"error": {"code": "hub_refused", "hint": text.chars().take(200).collect::<String>()}}));
    Ok((status, v))
}

/// `hello`: the tenant facts and the current epoch (cached).
pub(crate) fn hello(h: &HubRow) -> R<serde_json::Value> {
    let base = dial_base(h)?;
    hello_at(h, &base)
}

fn hello_at(h: &HubRow, base: &str) -> R<serde_json::Value> {
    let key = identity_key()?;
    let bytes = b"{}".to_vec();
    let sha = crypto::sha256_hex(&bytes);
    let (status, v) = post_signed(base, &key, "/cli/compute/hub/hello", "", bytes.into(), &sha, 0, Duration::from_secs(20)).map_err(|e| unreachable(h, &e))?;
    if status != 200 {
        return Err(refusal_from(status, &v));
    }
    if v.get("hubFingerprint").and_then(|f| f.as_str()).is_some_and(|f| f != h.fp) {
        return Err(no(hcodes::BAD_SIGNATURE, 502, format!("{} answered with another server's key; check the hub's address", h.label)));
    }
    if let Some(e) = v["epoch"].as_i64() {
        EPOCHS.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(HashMap::new).insert(h.fp.clone(), e);
    }
    Ok(v)
}

fn epoch_for(h: &HubRow) -> R<i64> {
    if let Some(e) = EPOCHS.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(&h.fp).copied()) {
        return Ok(e);
    }
    hello(h)?["epoch"].as_i64().ok_or_else(|| no("hub_refused", 502, "the hub's hello has no epoch"))
}

/// A signed JSON call to the hub. A stale epoch (the hub owner suspended
/// and resumed us) says hello again and retries once.
pub(crate) fn call(h: &HubRow, path: &str, body: &serde_json::Value, timeout: Duration) -> R<serde_json::Value> {
    let base = dial_base(h)?;
    let key = identity_key()?;
    let bytes = body.to_string().into_bytes();
    let sha = crypto::sha256_hex(&bytes);
    for attempt in 0..2 {
        let epoch = epoch_for(h)?;
        let (status, v) = post_signed(&base, &key, path, "", bytes.clone().into(), &sha, epoch, timeout).map_err(|e| unreachable(h, &e))?;
        if status == 200 {
            return Ok(v);
        }
        if attempt == 0 && v["error"]["code"] == hcodes::EPOCH_STALE {
            hello_at(h, &base)?;
            continue;
        }
        return Err(refusal_from(status, &v));
    }
    Err(no(hcodes::EPOCH_STALE, 409, "the hub's epoch kept changing"))
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Upload a file to the hub's blob route, streamed from disk (V21).
fn upload(h: &HubRow, file: &Path, sha: &str, bytes: u64, extra: &[(&str, &str)]) -> R<serde_json::Value> {
    let base = dial_base(h)?;
    let key = identity_key()?;
    let mut q = format!("sha256={sha}");
    for (k, v) in extra {
        q.push_str(&format!("&{k}={}", enc(v)));
    }
    let timeout = Duration::from_secs(120 + bytes / (256 << 10));
    for attempt in 0..2 {
        let epoch = epoch_for(h)?;
        let f = std::fs::File::open(file).map_err(|e| no("internal", 500, format!("open {}: {e}", file.display())))?;
        let body = reqwest::blocking::Body::sized(f, bytes);
        let (status, v) = post_signed(&base, &key, crate::compute_hub::BLOB_PATH, &q, body, sha, epoch, timeout).map_err(|e| unreachable(h, &e))?;
        if status == 200 {
            return Ok(v);
        }
        if attempt == 0 && v["error"]["code"] == hcodes::EPOCH_STALE {
            hello_at(h, &base)?;
            continue;
        }
        return Err(refusal_from(status, &v));
    }
    Err(no(hcodes::EPOCH_STALE, 409, "the hub's epoch kept changing"))
}

// ── config routes ────────────────────────────────────────────────────

pub(crate) fn handle(path: &str, who: &Who, body: &[u8]) -> R<serde_json::Value> {
    match path {
        "/cli/compute/hubs" => status(who),
        "/cli/compute/hubs/use" => use_hub(who, body),
        "/cli/compute/hubs/off" => hub_off(who),
        _ => Err(no("not_found", 404, "no such compute route")),
    }
}

fn status(who: &Who) -> R<serde_json::Value> {
    cr::need_admin(who)?;
    let (h, open) = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        (active_hub(&conn), open_remote_jobs(&conn).len())
    };
    let Some(h) = h else {
        return Ok(serde_json::json!({"hub": null, "openRemoteJobs": open, "note": "this server runs its own compute (no hub)"}));
    };
    let mut out = serde_json::json!({
        "hub": {"label": h.label, "fingerprint": h.fp, "baseUrl": h.base_url, "since": h.since, "setBy": h.set_by},
        "openRemoteJobs": open,
    });
    match hello(&h) {
        Ok(v) => {
            out["reachable"] = true.into();
            out["tenant"] = v;
            if let Ok(nodes) = call(&h, "/cli/compute/hub/nodes", &serde_json::json!({}), Duration::from_secs(20)) {
                out["nodes"] = nodes["nodes"].clone();
            }
        }
        Err(e) => {
            out["reachable"] = false.into();
            out["error"] = serde_json::json!({"code": e.code, "hint": e.hint});
        }
    }
    Ok(out)
}

/// `hubs/use {peer}`: owner only. The peer must be paired (Trusted, v2)
/// and must already have added this server as a tenant (hello answers).
fn use_hub(who: &Who, body: &[u8]) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let v = json_body(body)?;
    if !k2_core::federation::enabled() {
        return Err(no("federation_off", 409, "a compute hub is a paired server: turn federation on first (K2_FEDERATION=1 or Settings)"));
    }
    let sel = s(&v, "peer").ok_or_else(|| no(codes::BAD_REQUEST, 400, "name the hub (a paired server: fingerprint, label or host)"))?;
    let store = PeerStore::load().map_err(|e| no("internal", 500, format!("federation peers: {e}")))?;
    let peer = store
        .list()
        .iter()
        .find(|p| crate::federation_routes::peer_selector_matches(p, sel))
        .cloned()
        .ok_or_else(|| no("peer_not_found", 404, format!("no paired server '{sel}' (k2 fed peers)")))?;
    if peer.trust != k2_core::federation::PeerTrust::Trusted {
        return Err(no(hcodes::PEER_NOT_GRANTED, 409, format!("'{sel}' isn't confirmed (Trusted) yet: finish pairing first")));
    }
    if peer.pairing_version < k2_core::federation::pairing_v2::PAIRING_VERSION {
        return Err(no(hcodes::PAIRING_V2_REQUIRED, 409, format!("'{sel}' was paired with the old code; pair again (pairing v2) first")));
    }
    let base = crate::federation_routes::gated_peer_base_url(&peer).map_err(|e| no("peer_unreachable", 409, e))?;
    let label = if peer.label.is_empty() { peer.subdomain.clone() } else { peer.label.clone() };
    let row = HubRow { fp: peer.fingerprint.clone(), label, base_url: base.clone(), state: "active".into(), since: compute::now(), set_by: who.label() };
    // The hub must already know us.
    let tenant = hello_at(&row, &base)?;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        set_active(&conn, &row).map_err(|e| no("internal", 500, e))?;
        k2_core::compute::store::record_event(&conn, &who.label(), "hub_use", None, None, None, serde_json::json!({"hub": row.fp, "label": row.label}));
    }
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "compute-hub-use",
        &who.label(),
        format!("ok compute-hub-use hub={} fp={}", row.label, &row.fp[..16.min(row.fp.len())]),
        "loopback",
        "-",
        "cli",
    ));
    spawn_poller();
    Ok(serde_json::json!({"ok": true, "hub": {"label": row.label, "fingerprint": row.fp, "baseUrl": row.base_url}, "tenant": tenant,
        "note": "k2 compute run on this server now goes to the hub; jobs already queued here stay here"}))
}

fn hub_off(who: &Who) -> R<serde_json::Value> {
    cr::need_owner(who)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let was = active_hub(&conn);
    conn.execute("UPDATE compute_hubs SET state = 'off' WHERE state = 'active'", []).map_err(|e| no("internal", 500, e.to_string()))?;
    k2_core::compute::store::record_event(&conn, &who.label(), "hub_off", None, None, None, serde_json::json!({"hub": was.as_ref().map(|h| h.fp.clone())}));
    Ok(serde_json::json!({"ok": true, "was": was.map(|h| h.label),
        "note": "new runs use this server's own nodes again; jobs already on the hub keep reporting here"}))
}

// ── the proxy ────────────────────────────────────────────────────────

/// Paths whose `job` names one job (params or body).
const JOB_READS: &[&str] = &["/cli/compute/jobs/get", "/cli/compute/jobs/logs", "/cli/compute/jobs/artifact", "/cli/compute/jobs/receipt"];
const JOB_WRITES: &[&str] = &["/cli/compute/jobs/cancel", "/cli/compute/jobs/retry"];

/// Route a local compute request to the hub when it belongs there:
/// `run`/`jobs` (and an agent's `nodes`) while a hub is active, and any
/// verb on a job this server sent to a hub. `None` = handle it locally,
/// exactly as before.
pub(crate) fn intercept(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> Option<R<serde_json::Value>> {
    if who.tenant().is_some() {
        return None;
    }
    if JOB_READS.contains(&path) {
        let rj = remote_for(who, params.get("job").map(String::as_str))?;
        return Some(proxy_job_read(path, who, params, rj));
    }
    if JOB_WRITES.contains(&path) {
        let v = serde_json::from_slice::<serde_json::Value>(body).ok()?;
        let rj = remote_for(who, s(&v, "job"))?;
        return Some(proxy_job_write(path, who, v, rj));
    }
    let h = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        active_hub(&conn)
    }?;
    match path {
        "/cli/compute/run" => Some(proxy_run(who, body, &h)),
        "/cli/compute/jobs" => Some(proxy_jobs_list(who, params, &h)),
        // An agent sees the hub's nodes; the owner keeps this server's own
        // node list (`k2 compute hub status` shows the hub's).
        "/cli/compute/nodes" if who.passport().is_some() => Some(proxy_nodes(who, &h)),
        _ => None,
    }
}

/// The remote job `token` names, if this caller may see it. A passport
/// sees only its own workspace's (anything else falls through to the
/// local 404).
fn remote_for(who: &Who, token: Option<&str>) -> Option<RemoteJob> {
    let t = token?;
    let rj = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        remote_job_by_token(&conn, t)
    }?;
    match who.passport() {
        Some((ws, _)) if ws != rj.workspace_id => None,
        _ => Some(rj),
    }
}

fn hub_for(rj: &RemoteJob) -> R<HubRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    hub_by_fp(&conn, &rj.hub_fp).ok_or_else(|| no(codes::JOB_NOT_FOUND, 404, "that job's hub is gone from this server's settings"))
}

/// The hub's job JSON as this server shows it: this server's workspace
/// handle, and which hub.
fn localize(v: &mut serde_json::Value, rj_ws: Option<&str>, hub_label: &str) {
    if let Some(o) = v.as_object_mut() {
        let ws = rj_ws.map(str::to_string).or_else(|| o.get("remoteWsId").and_then(|x| x.as_str()).map(str::to_string));
        if let Some(ws) = ws {
            o.insert("workspace".into(), cr::ws_handle(&ws).into());
        }
        o.insert("hub".into(), hub_label.into());
    }
}

fn proxy_job_read(path: &str, who: &Who, params: &HashMap<String, String>, rj: RemoteJob) -> R<serde_json::Value> {
    cr::need_credential(who)?;
    let h = hub_for(&rj)?;
    let mut body = serde_json::Map::new();
    for (k, x) in params {
        if k != "token" {
            body.insert(k.clone(), x.clone().into());
        }
    }
    body.insert("job".into(), rj.hub_job_id.clone().into());
    body.insert("remoteWs".into(), rj.workspace_id.clone().into());
    compute_ws::touch_follower(&rj.hub_job_id);
    // An artifact may be pulled from the node first (up to minutes); the
    // reads long-poll at most 25 s.
    let timeout = if path == "/cli/compute/jobs/artifact" { Duration::from_secs(900) } else { Duration::from_secs(70) };
    let mut v = call(&h, &path.replacen("/cli/compute/", "/cli/compute/hub/", 1), &serde_json::Value::Object(body), timeout)?;
    compute_ws::touch_follower(&rj.hub_job_id);
    let state = if path == "/cli/compute/jobs/get" { v["state"].as_str().map(str::to_string) } else { v["job"]["state"].as_str().map(str::to_string) };
    if let Some(st) = state {
        let db = k2_core::db::shared();
        let conn = db.lock();
        update_remote(&conn, &rj.hub_job_id, &st, None, None, None);
    }
    if path == "/cli/compute/jobs/get" {
        localize(&mut v, Some(&rj.workspace_id), &h.label);
    } else if v.get("job").is_some_and(|j| j.is_object()) {
        localize(&mut v["job"], Some(&rj.workspace_id), &h.label);
    }
    Ok(v)
}

fn proxy_job_write(path: &str, who: &Who, mut v: serde_json::Value, rj: RemoteJob) -> R<serde_json::Value> {
    cr::need_credential(who)?;
    let h = hub_for(&rj)?;
    v["job"] = rj.hub_job_id.clone().into();
    v["remoteWs"] = rj.workspace_id.clone().into();
    let mut out = call(&h, &path.replacen("/cli/compute/", "/cli/compute/hub/", 1), &v, Duration::from_secs(30))?;
    // A retry reopens it: the poller watches it again.
    let state = out["job"]["state"].as_str().or(out["state"].as_str()).map(str::to_string);
    if let Some(st) = state {
        let db = k2_core::db::shared();
        let conn = db.lock();
        update_remote(&conn, &rj.hub_job_id, &st, None, None, None);
        if !is_terminal(&st) {
            let _ = conn.execute("UPDATE compute_remote_jobs SET notified_at = NULL, ended_at = NULL WHERE hub_job_id = ?1", params![rj.hub_job_id]);
        }
    }
    if out.get("job").is_some_and(|j| j.is_object()) {
        localize(&mut out["job"], Some(&rj.workspace_id), &h.label);
    }
    wake_poller();
    Ok(out)
}

fn proxy_nodes(who: &Who, h: &HubRow) -> R<serde_json::Value> {
    cr::need_credential(who)?;
    cr::passport_gate(who)?;
    let mut v = call(h, "/cli/compute/hub/nodes", &serde_json::json!({}), Duration::from_secs(20))?;
    v["hub"] = h.label.clone().into();
    Ok(v)
}

/// `jobs`: this server's own jobs (from before the hub) plus the hub's,
/// newest first. A passport sees only its workspace's.
fn proxy_jobs_list(who: &Who, params: &HashMap<String, String>, h: &HubRow) -> R<serde_json::Value> {
    let local = cr::jobs_list(who, params)?;
    let mut body = serde_json::Map::new();
    for k in ["node", "state", "limit"] {
        if let Some(x) = params.get(k) {
            body.insert(k.into(), x.clone().into());
        }
    }
    let ws = match who.passport() {
        Some((own, _)) => Some(own.to_string()),
        None => params.get("workspace").filter(|w| !w.is_empty()).and_then(|w| cr::ws_by_token_or_worktree(w)).map(|w| w.id),
    };
    if let Some(w) = &ws {
        body.insert("remoteWs".into(), w.clone().into());
    }
    if params.get("mine").is_some_and(|m| m == "1" || m == "true") {
        if let Some((_, sid)) = who.passport() {
            body.insert("remoteSession".into(), sid.into());
            body.insert("mine".into(), "1".into());
        }
    }
    let remote = call(h, "/cli/compute/hub/jobs", &serde_json::Value::Object(body), Duration::from_secs(30))?;
    let mut jobs: Vec<serde_json::Value> = local["jobs"].as_array().cloned().unwrap_or_default();
    for mut j in remote["jobs"].as_array().cloned().unwrap_or_default() {
        localize(&mut j, None, &h.label);
        jobs.push(j);
    }
    jobs.sort_by(|a, b| b["createdAt"].as_i64().cmp(&a["createdAt"].as_i64()));
    let limit = params.get("limit").and_then(|l| l.parse::<usize>().ok()).unwrap_or(30);
    jobs.truncate(limit.max(1));
    Ok(serde_json::json!({ "jobs": jobs, "hub": h.label }))
}

/// Up to this many recent ancestors of the commit are offered to `prepare`.
const PREPARE_COMMITS: usize = 256;

fn recent_commits(root: &Path, commit: &str) -> Vec<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["rev-list", &format!("--max-count={PREPARE_COMMITS}"), commit])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).lines().map(str::to_string).collect(),
        _ => vec![commit.to_string()],
    }
}

fn out_dir() -> PathBuf {
    compute::compute_dir().join("hub-out")
}

/// Make sure the hub has the code: the commit in its mirror (a
/// `have..commit` bundle when it lacks it), the `--dirty` blob in its
/// store. Returns the `source` the hub's `run` takes.
fn push_source(h: &HubRow, ws: &cr::Ws, v: &serde_json::Value, warnings: &mut Vec<String>) -> R<(serde_json::Value, cr::SourceInfo)> {
    let tag = crypto::hex(&crypto::random_bytes(8).map_err(|e| no("internal", 500, e))?);
    std::fs::create_dir_all(out_dir()).map_err(|e| no("internal", 500, format!("hub-out dir: {e}")))?;
    let dirty_out = out_dir().join(format!("{tag}.dirty.tar"));
    let (src, info) = cr::source_for(ws, v, &dirty_out, warnings)?;
    let root = compute::sync::repo_root(Path::new(&ws.path)).map_err(|e| no(codes::NOT_A_REPO, 400, e))?;
    let result = (|| -> R<serde_json::Value> {
        let remote = src.remote_url.clone();
        let commits = recent_commits(&root, &src.commit);
        let prep = call(
            h,
            "/cli/compute/hub/prepare",
            &serde_json::json!({"remoteWs": ws.id, "remote": remote, "commits": commits}),
            Duration::from_secs(30),
        )?;
        let have: Vec<String> = prep["have"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
        // The hub's caps for this door and our room in its blob store:
        // refuse here, before sending (the hub closes on a refused body).
        let max_blob = prep["maxBlobBytes"].as_u64().unwrap_or(hub::RELAY_MAX_BLOB_BYTES);
        let mut free = prep["blobFreeBytes"].as_u64().unwrap_or(u64::MAX);
        let mut room = |bytes: u64, what: &str| -> R<()> {
            if bytes > max_blob {
                return Err(no(
                    hcodes::BLOB_TOO_LARGE,
                    413,
                    format!("{what} is {} MB, over the hub's {} MB per upload on this route (a LAN route to the hub takes up to 1 GB)", bytes >> 20, max_blob >> 20),
                ));
            }
            if bytes > free {
                return Err(no(hcodes::BLOB_QUOTA, 409, format!("{what} is {} MB; this server has {} MB left in the hub's blob store", bytes >> 20, free >> 20)));
            }
            free = free.saturating_sub(bytes);
            Ok(())
        };
        if !have.contains(&src.commit) {
            let bundle = out_dir().join(format!("{tag}.bundle"));
            let mut send = |haves: &[String]| -> R<()> {
                compute::sync::create_bundle(&root, &src.commit, haves, &format!("up-{tag}"), &bundle).map_err(|e| no("sync_failed", 500, e))?;
                let (sha, bytes) = compute::sync::file_sha256(&bundle).map_err(|e| no("sync_failed", 500, e))?;
                if let Err(e) = room(bytes, "the code bundle") {
                    let _ = std::fs::remove_file(&bundle);
                    return Err(e);
                }
                let r = upload(
                    h,
                    &bundle,
                    &sha,
                    bytes,
                    &[("kind", "bundle"), ("remoteWs", &ws.id), ("remote", remote.as_deref().unwrap_or("")), ("commit", &src.commit)],
                );
                let _ = std::fs::remove_file(&bundle);
                r.map(|_| ())
            };
            match send(&have) {
                // The mirror changed under us (expired, re-made): send it all.
                Err(e) if e.code == "bundle_prereq_missing" => send(&[])?,
                other => other?,
            }
        }
        let dirty = match &src.dirty {
            Some(d) => {
                room(d.bytes, "the --dirty changes")?;
                upload(h, &dirty_out, &d.sha256, d.bytes, &[("kind", "dirty")])?;
                Some(serde_json::json!({"sha256": d.sha256, "bytes": d.bytes}))
            }
            None => None,
        };
        Ok(serde_json::json!({"commit": src.commit, "remote": remote, "dirty": dirty}))
    })();
    let _ = std::fs::remove_file(&dirty_out);
    result.map(|s| (s, info))
}

fn proxy_run(who: &Who, body: &[u8], h: &HubRow) -> R<serde_json::Value> {
    cr::need_credential(who)?;
    cr::passport_gate(who)?;
    let mut v = json_body(body)?;
    let ws = cr::target_ws(who, s(&v, "workspace"))?;
    let session = who.passport().map(|(_, sid)| sid.to_string());
    let plan = b(&v, "plan") == Some(true);
    let mut warnings: Vec<String> = Vec::new();
    let mut info = None;
    if !plan && b(&v, "noSource") != Some(true) {
        let (source, i) = push_source(h, &ws, &v, &mut warnings)?;
        v["source"] = source;
        info = Some(i);
    }
    if let Some(o) = v.as_object_mut() {
        for k in ["workspace", "repoDir", "sha", "dirty", "remoteWs", "remoteWsLabel", "remoteSession"] {
            o.remove(k);
        }
        o.insert("remoteWs".into(), ws.id.clone().into());
        o.insert("remoteWsLabel".into(), ws.handle.clone().into());
        if let Some(sid) = &session {
            o.insert("remoteSession".into(), sid.clone().into());
        }
    }
    let mut out = call(h, "/cli/compute/hub/run", &v, Duration::from_secs(60))?;
    if plan {
        out["hub"] = h.label.clone().into();
        return Ok(out);
    }
    let detach = b(&v, "detach") == Some(true);
    let client = s(&v, "clientJobId").unwrap_or_default().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(id) = out["job"]["id"].as_str() {
            record_remote(&conn, id, &h.fp, &ws.id, session.as_deref(), &client, detach, out["job"]["state"].as_str().unwrap_or("queued"), out["job"]["node"].as_str());
        }
        // A split run's shards: their logs and files are read by id, and
        // they report to the run (no message of their own).
        for sh in out["shards"].as_array().cloned().unwrap_or_default() {
            if let Some(id) = sh["job"].as_str() {
                record_remote(&conn, id, &h.fp, &ws.id, None, &format!("{client}-shard"), false, sh["state"].as_str().unwrap_or("queued"), sh["node"].as_str());
            }
        }
    }
    if out.get("job").is_some_and(|j| j.is_object()) {
        localize(&mut out["job"], Some(&ws.id), &h.label);
    }
    if let Some(i) = info {
        out["source"] = i.json();
    }
    let mut all: Vec<serde_json::Value> = warnings.into_iter().map(serde_json::Value::from).collect();
    all.extend(out["warnings"].as_array().cloned().unwrap_or_default());
    out["warnings"] = all.into();
    out["hub"] = h.label.clone().into();
    spawn_poller();
    wake_poller();
    Ok(out)
}

// ── the poller (V24) ─────────────────────────────────────────────────

static POLLER: Once = Once::new();
static WAKE: (Mutex<u64>, Condvar) = (Mutex::new(0), Condvar::new());

fn wake_poller() {
    let (m, c) = &WAKE;
    *m.lock().unwrap_or_else(|p| p.into_inner()) += 1;
    c.notify_all();
}

fn sleep_or_wake(d: Duration) {
    let (m, c) = &WAKE;
    let g = m.lock().unwrap_or_else(|p| p.into_inner());
    let seen = *g;
    let _ = c.wait_timeout_while(g, d, |n| *n == seen);
}

/// Start the one poller (idempotent): daemon boot and every proxied run.
pub fn spawn_poller() {
    POLLER.call_once(|| {
        let _ = std::thread::Builder::new().name("compute-hub-poller".into()).spawn(|| loop {
            let idle = match poll_once() {
                Ok(true) => Duration::from_millis(50),
                Ok(false) => Duration::from_secs(10),
                Err(()) => Duration::from_secs(10),
            };
            sleep_or_wake(idle);
        });
    });
}

/// One pass: every hub with open jobs, one `changes` long-poll each.
/// `Ok(true)` = there was something to watch; `Err` = a hub didn't answer.
fn poll_once() -> Result<bool, ()> {
    if !compute::enabled() {
        return Ok(false);
    }
    let open = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        open_remote_jobs(&conn)
    };
    if open.is_empty() {
        return Ok(false);
    }
    let mut by_hub: HashMap<String, Vec<RemoteJob>> = HashMap::new();
    for j in open {
        by_hub.entry(j.hub_fp.clone()).or_default().push(j);
    }
    let mut failed = false;
    for (fp, jobs) in by_hub {
        let h = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            hub_by_fp(&conn, &fp)
        };
        let Some(h) = h else { continue };
        let list: Vec<serde_json::Value> = jobs.iter().map(|j| serde_json::json!({"id": j.hub_job_id, "state": j.state})).collect();
        match call(&h, "/cli/compute/hub/changes", &serde_json::json!({"jobs": list, "wait": 20}), Duration::from_secs(45)) {
            Ok(v) => apply_changes(&jobs, &v),
            Err(e) if e.code == hcodes::TENANT_REVOKED || e.code == hcodes::PEER_NOT_GRANTED || e.code == hcodes::TENANT_UNKNOWN => {
                // The hub dropped this server: its open jobs end here, once.
                for j in &jobs {
                    end_lost(j, &format!("the hub {} no longer serves this server ({})", h.label, e.code));
                }
            }
            Err(_) => failed = true,
        }
    }
    if failed {
        Err(())
    } else {
        Ok(true)
    }
}

fn apply_changes(jobs: &[RemoteJob], v: &serde_json::Value) {
    let by_id: HashMap<&str, &RemoteJob> = jobs.iter().map(|j| (j.hub_job_id.as_str(), j)).collect();
    for c in v["jobs"].as_array().cloned().unwrap_or_default() {
        let Some(rj) = c["id"].as_str().and_then(|id| by_id.get(id)) else { continue };
        let state = c["state"].as_str().unwrap_or("unknown").to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            update_remote(&conn, &rj.hub_job_id, &state, c["node"].as_str(), Some(&c.to_string()), c["endedAt"].as_i64());
        }
        if is_terminal(&state) {
            deliver_if_due(rj, &c);
        }
    }
    for id in v["unknown"].as_array().cloned().unwrap_or_default() {
        if let Some(rj) = id.as_str().and_then(|i| by_id.get(i)) {
            end_lost(rj, "the hub no longer knows this job");
        }
    }
}

fn end_lost(rj: &RemoteJob, why: &str) {
    let c = serde_json::json!({"id": rj.hub_job_id, "state": "unknown", "reason": "hub_lost", "detail": why, "node": rj.node});
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        update_remote(&conn, &rj.hub_job_id, "unknown", None, Some(&c.to_string()), Some(compute::now()));
    }
    deliver_if_due(rj, &c);
}

/// The local `[compute]` message for a remote job that ended, once:
/// detached jobs always; attached ones when nobody follows them any more
/// (the same rule as this server's own jobs).
fn deliver_if_due(rj: &RemoteJob, c: &serde_json::Value) {
    let wants = rj.detach || (rj.session_id.is_some() && !compute_ws::follower_recent(&rj.hub_job_id));
    if !wants {
        return;
    }
    let ws_path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if !mark_notified(&conn, &rj.hub_job_id) {
            return;
        }
        k2_core::db::schema::Project::get(&conn, &rj.workspace_id).ok().map(|p| p.path)
    };
    let started = c["startedAt"].as_i64().or(c["assignedAt"].as_i64());
    let secs = match (started, c["endedAt"].as_i64()) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    };
    let tests: Option<k2_core::compute::proto::results::TestSummary> = serde_json::from_value(c["tests"].clone()).ok();
    let node = c["node"].as_str().or(rj.node.as_deref()).unwrap_or("the hub").to_string();
    let state = c["state"].as_str().filter(|x| JobState::parse(x).is_some()).unwrap_or("unknown");
    // Facts only: a reason is a short code, never text from elsewhere.
    let reason = c["reason"].as_str().filter(|r| (1..=48).contains(&r.len()) && r.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_'));
    let node: String = node.chars().filter(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(*ch)).take(64).collect();
    let shards = n(c, "shards").unwrap_or(0).max(0) as usize;
    let text = if shards > 0 {
        compute::message::shards_completion_text(&rj.hub_job_id, shards, state, n(c, "exitCode"), n(c, "signal"), reason, secs, tests.as_ref())
    } else {
        compute::message::completion_text(&rj.hub_job_id, &node, state, n(c, "exitCode"), n(c, "signal"), reason, secs, tests.as_ref())
    };
    let (id, session) = (rj.hub_job_id.clone(), rj.session_id.clone());
    std::thread::spawn(move || compute_ws::deliver_completion(&id, session.as_deref(), ws_path.as_deref(), &text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};

    /// A fake hub: verifies every request's X-K2-Hub signature against the
    /// submitter's own federation key, records (path, body), and answers
    /// per path. No real network beyond loopback, no real keys.
    struct FakeHub {
        port: u16,
        seen: std::sync::Arc<Mutex<Vec<(String, serde_json::Value)>>>,
    }

    fn fake_hub(spki: Vec<u8>, answer: fn(&str, &serde_json::Value) -> (u16, serde_json::Value)) -> FakeHub {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
        let rec = seen.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(mut s) = s else { break };
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let pq = line.split_whitespace().nth(1).unwrap_or("").to_string();
                let (path, query) = pq.split_once('?').map(|(a, b)| (a.to_string(), b.to_string())).unwrap_or((pq.clone(), String::new()));
                let (mut len, mut auth) = (0usize, None);
                loop {
                    let mut h = String::new();
                    r.read_line(&mut h).unwrap();
                    if h == "\r\n" || h.is_empty() {
                        break;
                    }
                    let (k, v) = h.split_once(':').unwrap();
                    if k.eq_ignore_ascii_case("content-length") {
                        len = v.trim().parse().unwrap();
                    }
                    if k.eq_ignore_ascii_case(hub::HEADER) {
                        auth = hub::HubAuth::parse(v.trim());
                    }
                }
                let mut body = vec![0u8; len];
                r.read_exact(&mut body).unwrap();
                let a = auth.expect("every hub request is signed");
                let body_sha = if path == crate::compute_hub::BLOB_PATH {
                    k2_core::agent_hooks::parse_query_params(&pq).get("sha256").cloned().unwrap()
                } else {
                    crypto::sha256_hex(&body)
                };
                assert!(hub::signature_ok(&a, &spki, "POST", &path, &query, &body_sha), "signature verifies for {path}");
                assert_eq!(crypto::sha256_hex(&body), body_sha, "the body is what was signed");
                let v: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::json!({"bytes": body.len()}));
                rec.lock().unwrap().push((path.clone(), v.clone()));
                let (status, out) = answer(&path, &v);
                let text = out.to_string();
                let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len());
            }
        });
        FakeHub { port, seen }
    }

    fn answers(path: &str, v: &serde_json::Value) -> (u16, serde_json::Value) {
        match path {
            "/cli/compute/hub/hello" => (200, serde_json::json!({"tenant": "tn-x", "label": "me", "epoch": 7})),
            "/cli/compute/hub/jobs/get" if v["job"] == "missing" => (404, serde_json::json!({"error": {"code": "job_not_found", "hint": "no job 'missing'"}})),
            "/cli/compute/hub/jobs/get" => (200, serde_json::json!({"id": v["job"], "state": "running", "remoteWsId": v["remoteWs"]})),
            "/cli/compute/hub/run" => (200, serde_json::json!({"job": {"id": "abcdef0123456789abcdef0123456789", "state": "queued", "node": "hn1"}, "existing": false})),
            "/cli/compute/hub/weird" => (418, serde_json::json!({"error": {"code": "teapot", "hint": "short and stout"}})),
            _ => (404, serde_json::json!({"error": {"code": "not_found", "hint": "no"}})),
        }
    }

    #[test]
    fn calls_are_signed_round_trip_and_map_refusals() {
        crate::test_support::with_temp_home(|| {
            let key = identity_key().unwrap();
            let fake = fake_hub(key.spki_der().to_vec(), answers);
            let h = HubRow {
                fp: key.fingerprint(),
                label: "fakehub".into(),
                base_url: format!("http://127.0.0.1:{}", fake.port),
                state: "active".into(),
                since: 1,
                set_by: "owner".into(),
            };
            // hello learns the epoch; the next call carries it.
            let r = call(&h, "/cli/compute/hub/run", &serde_json::json!({"argv": ["true"], "remoteWs": "w1"}), Duration::from_secs(5)).unwrap();
            assert_eq!(r["job"]["node"], "hn1");
            let seen = fake.seen.lock().unwrap().clone();
            assert_eq!(seen.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), vec!["/cli/compute/hub/hello", "/cli/compute/hub/run"]);
            assert_eq!(seen[1].1["remoteWs"], "w1");
            // A hub refusal keeps its code and status.
            let e = call(&h, "/cli/compute/hub/jobs/get", &serde_json::json!({"job": "missing"}), Duration::from_secs(5)).unwrap_err();
            assert_eq!((e.code, e.status), ("job_not_found", 404));
            // An unknown code is carried in the hint, never invented.
            let e = call(&h, "/cli/compute/hub/weird", &serde_json::json!({}), Duration::from_secs(5)).unwrap_err();
            assert_eq!((e.code, e.status), ("hub_refused", 418));
            assert!(e.hint.contains("teapot"), "{}", e.hint);
            // A blob upload is signed over its sha256 and streamed from disk.
            let f = std::env::temp_dir().join(format!("k2-blob-test-{}", std::process::id()));
            std::fs::write(&f, b"blob bytes").unwrap();
            let (sha, n) = compute::sync::file_sha256(&f).unwrap();
            let e = upload(&h, &f, &sha, n, &[("kind", "dirty")]).unwrap_err();
            assert_eq!(e.code, "not_found", "the fake hub has no blob route; the request still verified");
            let _ = std::fs::remove_file(&f);
        });
    }

    #[test]
    fn a_hub_that_is_down_fails_fast_with_hub_unreachable() {
        crate::test_support::with_temp_home(|| {
            // A port nobody listens on.
            let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
            let h = HubRow { fp: "f".repeat(64), label: "gone".into(), base_url: format!("http://127.0.0.1:{port}"), state: "active".into(), since: 1, set_by: "o".into() };
            let t = std::time::Instant::now();
            let e = call(&h, "/cli/compute/hub/jobs", &serde_json::json!({}), Duration::from_secs(5)).unwrap_err();
            assert_eq!((e.code, e.status), (hcodes::HUB_UNREACHABLE, 503));
            assert!(e.hint.contains("No automatic fallback"), "{}", e.hint);
            assert!(t.elapsed() < Duration::from_secs(10), "fails fast");
        });
    }

    #[test]
    fn the_hub_answering_with_another_key_is_refused() {
        crate::test_support::with_temp_home(|| {
            let key = identity_key().unwrap();
            fn other_hello(_: &str, _: &serde_json::Value) -> (u16, serde_json::Value) {
                (200, serde_json::json!({"epoch": 1, "hubFingerprint": "0".repeat(64)}))
            }
            let fake = fake_hub(key.spki_der().to_vec(), other_hello);
            let h = HubRow { fp: "a".repeat(64), label: "x".into(), base_url: format!("http://127.0.0.1:{}", fake.port), state: "active".into(), since: 1, set_by: "o".into() };
            let e = hello(&h).unwrap_err();
            assert_eq!(e.code, hcodes::BAD_SIGNATURE);
        });
    }

    #[test]
    fn remote_job_rows_resolve_short_ids_and_notify_once() {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let conn = db.lock();
        record_remote(&conn, "0123456789abcdef0123456789abcdef", "fp", "ws-1", Some("s1"), "c1", true, "queued", None);
        record_remote(&conn, "0123ffff89abcdef0123456789abcdef", "fp", "ws-1", None, "c2", false, "running", None);
        assert_eq!(remote_job_by_token(&conn, "01234567").unwrap().client_job_id, "c1");
        assert!(remote_job_by_token(&conn, "0123").is_none(), "ambiguous prefix");
        assert_eq!(open_remote_jobs(&conn).len(), 2);
        update_remote(&conn, "0123456789abcdef0123456789abcdef", "done", Some("hn1"), Some("{}"), Some(9));
        assert_eq!(open_remote_jobs(&conn).len(), 1, "a terminal job isn't polled");
        assert!(mark_notified(&conn, "0123456789abcdef0123456789abcdef"));
        assert!(!mark_notified(&conn, "0123456789abcdef0123456789abcdef"), "delivered once");
        // Re-recording the same id never resets it.
        record_remote(&conn, "0123456789abcdef0123456789abcdef", "fp", "ws-2", None, "c9", false, "queued", None);
        assert_eq!(remote_job(&conn, "0123456789abcdef0123456789abcdef").unwrap().workspace_id, "ws-1");
    }

    #[test]
    fn one_active_hub_at_a_time() {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mk = |fp: &str| HubRow { fp: fp.into(), label: fp.into(), base_url: "http://h".into(), state: "active".into(), since: 1, set_by: "o".into() };
        assert!(active_hub(&conn).is_none(), "no hub: this server runs its own compute");
        set_active(&conn, &mk("a")).unwrap();
        set_active(&conn, &mk("b")).unwrap();
        assert_eq!(active_hub(&conn).unwrap().fp, "b");
        assert_eq!(hub_by_fp(&conn, "a").unwrap().state, "off");
    }
}
