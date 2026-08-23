//! Durable host-session spawn queue (prd-host-session-spawn-queue-v1).
//!
//! When a workspace (or principal / daemon) is at its concurrent host-session
//! ceiling, excess cold / dead-resume spawns are **enqueued** into a
//! path-keyed FIFO and **automatically started** on every quota
//! [`crate::sandbox_quota::release_in_workspace`] (not ChildExit-only).
//!
//! ## Product locks
//! - Feature **default OFF** (`K2_HOST_SESSION_SPAWN_QUEUE` / 0).
//! - Feature ON admit: always **nowait** acquire; refuse → enqueue (or
//!   immediate 429 when `queue:false` / depth full).
//! - Feature OFF: legacy S8 wait then 429 only (byte-compatible).
//! - Mint capability JWTs only at drain/spawn time (specs stored, not JWTs).
//! - Drain calls [`crate::v1_host_sessions::spawn_host_session_after_acquire`]
//!   — never re-enters full `handle_v1_host_new` (no double-acquire).
//! - Prompt never logged; purged on terminal states.
//!
//! ## Store
//! SQLite table `host_session_spawn_queue` (migration 0096) in the daemon DB
//! (0600 via box DB permissions). Survives daemon restart (A9); max-age
//! jobs expire on drain/list.

use std::cell::Cell;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use k2_core::log_debug;

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;
use crate::sandbox_quota;
use crate::v1_host_sessions::policy::ApiHostSessionRequest;

// ── Feature / limits ─────────────────────────────────────────────────

/// Default max queued jobs per workspace path.
const DEFAULT_QUEUE_DEPTH: usize = 32;
/// Default max queued jobs across all workspaces.
const DEFAULT_GLOBAL_QUEUE_DEPTH: usize = 256;
/// Default max job age before expire (20 minutes).
const DEFAULT_MAX_AGE_SECS: u64 = 20 * 60;

/// Is the durable spawn queue feature enabled?
/// Env `K2_HOST_SESSION_SPAWN_QUEUE`: `1`/`true`/`on`/`yes` → ON; default OFF.
pub fn feature_enabled() -> bool {
    match std::env::var("K2_HOST_SESSION_SPAWN_QUEUE") {
        Ok(v) => {
            let t = v.trim().to_ascii_lowercase();
            matches!(t.as_str(), "1" | "true" | "on" | "yes")
        }
        Err(_) => false,
    }
}

fn queue_depth() -> usize {
    env_usize("K2_HOST_SESSION_QUEUE_DEPTH", DEFAULT_QUEUE_DEPTH)
}

fn global_queue_depth() -> usize {
    env_usize(
        "K2_HOST_SESSION_QUEUE_GLOBAL_DEPTH",
        DEFAULT_GLOBAL_QUEUE_DEPTH,
    )
}

fn max_age_secs() -> u64 {
    match std::env::var("K2_HOST_SESSION_QUEUE_MAX_AGE_SECS") {
        Ok(v) => v
            .trim()
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(DEFAULT_MAX_AGE_SECS),
        Err(_) => DEFAULT_MAX_AGE_SECS,
    }
}

fn env_usize(var: &str, default: usize) -> usize {
    match std::env::var(var) {
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .ok()
            .filter(|n| *n > 0)
            .unwrap_or(default),
        Err(_) => default,
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn mint_job_id() -> String {
    format!("hsq_{}", uuid::Uuid::new_v4())
}

// ── Job model ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Cold,
    DeadResume,
}

impl JobKind {
    pub fn as_str(self) -> &'static str {
        match self {
            JobKind::Cold => "cold",
            JobKind::DeadResume => "dead_resume",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "cold" => Some(JobKind::Cold),
            "dead_resume" => Some(JobKind::DeadResume),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Expired,
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            JobStatus::Queued => "queued",
            JobStatus::Running => "running",
            JobStatus::Completed => "completed",
            JobStatus::Failed => "failed",
            JobStatus::Expired => "expired",
            JobStatus::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(JobStatus::Queued),
            "running" => Some(JobStatus::Running),
            "completed" => Some(JobStatus::Completed),
            "failed" => Some(JobStatus::Failed),
            "expired" => Some(JobStatus::Expired),
            "cancelled" => Some(JobStatus::Cancelled),
            _ => None,
        }
    }

    fn is_terminal(self) -> bool {
        matches!(
            self,
            JobStatus::Completed
                | JobStatus::Failed
                | JobStatus::Expired
                | JobStatus::Cancelled
        )
    }
}

/// Wire-facing job snapshot (prompt never included).
#[derive(Debug, Clone)]
pub struct JobView {
    pub job_id: String,
    pub status: JobStatus,
    pub workspace_slug: String,
    /// Path key (internal; wire uses slug).
    #[allow(dead_code)]
    pub workspace_path: String,
    pub principal_id: String,
    pub kind: JobKind,
    pub session_id: Option<String>,
    pub client_request_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub result_session_id: Option<String>,
    pub fail_code: Option<String>,
    pub fail_message: Option<String>,
    /// 1-based FIFO position among still-queued jobs in this workspace
    /// (None when not queued).
    pub position: Option<usize>,
}

/// Payload needed to enqueue (prompt kept only in the job store).
#[derive(Debug, Clone)]
pub struct EnqueueRequest {
    pub workspace_path: String,
    pub workspace_slug: String,
    pub principal_id: String,
    pub kind: JobKind,
    pub session_id: Option<String>,
    pub prompt: Option<String>,
    pub timeout_secs: Option<u64>,
    pub capabilities: Option<serde_json::Value>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    pub client_request_id: Option<String>,
    /// API model override persisted across enqueue → drain (D8).
    pub model: Option<String>,
}

#[derive(Debug)]
pub enum EnqueueError {
    DepthFull,
    /// Dead-resume already has an open job for this session.
    DeadResumeDuplicate { job_id: String, position: usize },
}

// ── Re-entrancy guard (drain inside release) ─────────────────────────

thread_local! {
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

/// Serialize drain across threads so two ChildExits don't double-pop the head.
static DRAIN_LOCK: Mutex<()> = Mutex::new(());

// Optional test hook: replace real spawn with a mock (unit tests).
#[cfg(test)]
static TEST_SPAWN_HOOK: Mutex<Option<Box<dyn Fn(&SpawnJob) -> Result<String, String> + Send>>> =
    Mutex::new(None);

/// Internal job row used by drain (includes prompt + specs).
struct SpawnJob {
    job_id: String,
    workspace_path: String,
    workspace_slug: String,
    principal_id: String,
    kind: JobKind,
    session_id: Option<String>,
    prompt: Option<String>,
    timeout_secs: Option<u64>,
    capabilities: Option<serde_json::Value>,
    cols: Option<u16>,
    rows: Option<u16>,
    model: Option<String>,
}

// ── Public API ───────────────────────────────────────────────────────

/// Enqueue a cold / dead-resume job. Returns (job_id, 1-based position).
/// Prompt is stored only here; never logged.
pub fn enqueue(req: EnqueueRequest) -> Result<(String, usize), EnqueueError> {
    expire_stale_for_workspace(&req.workspace_path);

    // Idempotency: same clientRequestId while open → same jobId.
    if let Some(ref crid) = req.client_request_id {
        if let Some(existing) = find_open_by_client_request(
            &req.workspace_path,
            &req.principal_id,
            crid,
        ) {
            let pos = position_of(&req.workspace_path, &existing).unwrap_or(1);
            return Ok((existing, pos));
        }
    }

    // Dead-resume: at most one open job per (ws, sessionId).
    if req.kind == JobKind::DeadResume {
        if let Some(ref sid) = req.session_id {
            if let Some(existing) = find_open_dead_resume(&req.workspace_path, sid) {
                let pos = position_of(&req.workspace_path, &existing).unwrap_or(1);
                return Err(EnqueueError::DeadResumeDuplicate {
                    job_id: existing,
                    position: pos,
                });
            }
        }
    }

    let ws_depth = count_queued(&req.workspace_path);
    if ws_depth >= queue_depth() {
        return Err(EnqueueError::DepthFull);
    }
    let global = count_queued_global();
    if global >= global_queue_depth() {
        return Err(EnqueueError::DepthFull);
    }

    let job_id = mint_job_id();
    let now = now_secs();
    let caps_json = req
        .capabilities
        .as_ref()
        .map(|v| v.to_string());
    // Never log prompt.
    log_debug!(
        "[spawn-queue] enqueue job={} ws={} kind={} principal={}",
        job_id,
        req.workspace_slug,
        req.kind.as_str(),
        req.principal_id
    );

    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "INSERT INTO host_session_spawn_queue \
         (job_id, workspace_path, workspace_slug, principal_id, kind, session_id, \
          prompt, timeout_secs, capabilities_json, cols, rows, client_request_id, \
          status, created_at, updated_at, model) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'queued',?13,?13,?14)",
        rusqlite::params![
            job_id,
            req.workspace_path,
            req.workspace_slug,
            req.principal_id,
            req.kind.as_str(),
            req.session_id,
            req.prompt,
            req.timeout_secs.map(|t| t as i64),
            caps_json,
            req.cols.map(|c| c as i64),
            req.rows.map(|r| r as i64),
            req.client_request_id,
            now,
            req.model,
        ],
    )
    .expect("spawn queue insert");

    let pos = position_of(&req.workspace_path, &job_id).unwrap_or(ws_depth + 1);
    Ok((job_id, pos))
}

/// Build the 202 Accepted response body for an enqueued job.
pub fn queued_response(
    job_id: &str,
    position: usize,
    workspace_slug: &str,
    session_id: Option<&str>,
) -> CliResponse {
    let mut body = serde_json::json!({
        "queued": true,
        "jobId": job_id,
        "workspace": workspace_slug,
        "position": position,
        "createdAt": now_secs(),
    });
    // Dead-resume may echo the claimed sessionId (not proof of live).
    if let Some(sid) = session_id {
        body["sessionId"] = serde_json::json!(sid);
    }
    CliResponse {
        status: "202 Accepted",
        content_type: "application/json",
        body: body.to_string(),
    }
}

/// 429 spawn-queue-full.
pub fn queue_full_response() -> CliResponse {
    CliResponse {
        status: "429 Too Many Requests",
        content_type: "application/json",
        body: serde_json::json!({
            "error": "host-session spawn queue is full for this workspace",
            "code": "spawn-queue-full",
        })
        .to_string(),
    }
}

/// GET job status (owner principal only — caller enforces).
pub fn get_job(workspace_path: &str, job_id: &str) -> Option<JobView> {
    expire_stale_for_workspace(workspace_path);
    load_job_view(workspace_path, job_id)
}

/// List open (queued|running) jobs for a workspace, owner-filtered by principal.
pub fn list_jobs(workspace_path: &str, principal_id: &str) -> Vec<JobView> {
    expire_stale_for_workspace(workspace_path);
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT job_id FROM host_session_spawn_queue \
         WHERE workspace_path = ?1 AND principal_id = ?2 \
           AND status IN ('queued','running') \
         ORDER BY rowid ASC \
         LIMIT 64",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let ids: Vec<String> = stmt
        .query_map(rusqlite::params![workspace_path, principal_id], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default();
    drop(stmt);
    drop(conn);
    ids.into_iter()
        .filter_map(|id| load_job_view(workspace_path, &id))
        .collect()
}

/// Cancel a still-queued job. Owning principal only (caller checks).
/// Returns true if cancelled, false if not found / not cancellable.
pub fn cancel_job(
    workspace_path: &str,
    job_id: &str,
    principal_id: &str,
) -> Result<JobView, CancelError> {
    let Some(view) = load_job_view(workspace_path, job_id) else {
        return Err(CancelError::NotFound);
    };
    if view.principal_id != principal_id {
        return Err(CancelError::NotFound); // uniform 404 — no existence oracle
    }
    if view.status != JobStatus::Queued {
        return Err(CancelError::NotCancellable { status: view.status });
    }
    mark_terminal(
        job_id,
        JobStatus::Cancelled,
        None,
        Some("cancelled"),
        Some("cancelled by caller"),
    );
    load_job_view(workspace_path, job_id).ok_or(CancelError::NotFound)
}

#[derive(Debug)]
pub enum CancelError {
    NotFound,
    NotCancellable { status: JobStatus },
}

/// Called from **every** [`sandbox_quota::release_in_workspace`].
/// Drains the workspace FIFO (or scans all non-empty queues when
/// `workspace` is `None`). Strict no-skip: if head cannot acquire, stop.
pub fn on_slot_freed(_principal: Option<&str>, workspace: Option<&str>) {
    if !feature_enabled() {
        return;
    }
    // Suppress re-entry while a drain is already active on this thread
    // (spawn-fail → release → would re-enter).
    if DRAINING.with(|d| d.get()) {
        return;
    }
    let Ok(_guard) = DRAIN_LOCK.lock() else {
        return;
    };
    DRAINING.with(|d| d.set(true));
    if let Some(ws) = workspace {
        drain_workspace(ws);
    } else {
        // Best-effort: scan workspaces with queued jobs.
        for ws in workspaces_with_queued() {
            drain_workspace(&ws);
        }
    }
    DRAINING.with(|d| d.set(false));
}

// ── Drain ────────────────────────────────────────────────────────────

fn drain_workspace(workspace_path: &str) {
    expire_stale_for_workspace(workspace_path);
    loop {
        let Some(head) = peek_head(workspace_path) else {
            break;
        };
        // Reload principal at drain time.
        let principal = match reload_principal(&head.principal_id) {
            Some(p) => p,
            None => {
                log_debug!(
                    "[spawn-queue] principal-gone job={} principal={}",
                    head.job_id,
                    head.principal_id
                );
                // No slot held yet — just fail the job and continue.
                mark_terminal(
                    &head.job_id,
                    JobStatus::Failed,
                    None,
                    Some("principal-gone"),
                    Some("api key missing, revoked, or disabled at drain"),
                );
                continue;
            }
        };

        let ws_cell_cap =
            k2_core::workspace::settings::get_host_session_cell_cap(&head.workspace_path);
        match sandbox_quota::try_acquire_in_workspace_with_cap_nowait(
            &head.principal_id,
            Some(&head.workspace_path),
            ws_cell_cap,
        ) {
            Ok(()) => {
                // Pop head → running, then spawn_after_acquire (no second acquire).
                // Keep prompt until terminal mark (spawn still needs it).
                if !claim_running(&head.job_id) {
                    // Lost race — release the slot we just took.
                    sandbox_quota::release_in_workspace(
                        &head.principal_id,
                        Some(&head.workspace_path),
                    );
                    break;
                }
                match run_spawn(&principal, &head) {
                    Ok(session_id) => {
                        mark_terminal(
                            &head.job_id,
                            JobStatus::Completed,
                            Some(&session_id),
                            None,
                            None,
                        );
                        log_debug!(
                            "[spawn-queue] completed job={} session={}",
                            head.job_id,
                            session_id
                        );
                        // Slot remains held by the live cell. Further heads
                        // only start if more capacity exists (nowait).
                        continue;
                    }
                    Err((code, msg)) => {
                        log_debug!(
                            "[spawn-queue] spawn failed job={} code={}",
                            head.job_id,
                            code
                        );
                        mark_terminal(
                            &head.job_id,
                            JobStatus::Failed,
                            None,
                            Some(&code),
                            Some(&msg),
                        );
                        // Release the held slot; re-entry suppressed (DRAINING).
                        sandbox_quota::release_in_workspace(
                            &head.principal_id,
                            Some(&head.workspace_path),
                        );
                        continue;
                    }
                }
            }
            Err(_) => {
                // Strict FIFO: head cannot acquire → stop (no skip).
                break;
            }
        }
    }
}

fn run_spawn(principal: &V1Principal, job: &SpawnJob) -> Result<String, (String, String)> {
    #[cfg(test)]
    {
        if let Ok(hook) = TEST_SPAWN_HOOK.lock() {
            if let Some(ref f) = *hook {
                return f(job).map_err(|e| ("spawn-failed".into(), e));
            }
        }
    }

    let req = ApiHostSessionRequest {
        prompt: job.prompt.clone(),
        cols: job.cols,
        rows: job.rows,
        timeout_secs: job.timeout_secs,
        session: job.session_id.clone(),
        capabilities: job.capabilities.clone(),
        queue: None,
        client_request_id: None,
        model: job.model.clone(),
    };
    let is_resume = job.kind == JobKind::DeadResume;
    let resume_target = job.session_id.clone();
    let resp = crate::v1_host_sessions::spawn_host_session_after_acquire(
        principal,
        &job.workspace_path,
        &job.workspace_slug,
        &req,
        is_resume,
        resume_target.as_deref(),
    );
    // completed = cell started + sessionId (not agent --final).
    if resp.status.starts_with("200") {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&resp.body) {
            if let Some(sid) = v.get("sessionId").and_then(|x| x.as_str()) {
                if !sid.is_empty() {
                    return Ok(sid.to_string());
                }
            }
        }
        return Err((
            "spawn-response".into(),
            "spawn succeeded but no sessionId".into(),
        ));
    }
    let parsed = serde_json::from_str::<serde_json::Value>(&resp.body).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()))
        .unwrap_or("spawn-failed")
        .to_string();
    let msg = parsed
        .as_ref()
        .and_then(|v| v.get("error").and_then(|e| e.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| resp.body.clone());
    Err((code, msg))
}

fn reload_principal(principal_id: &str) -> Option<V1Principal> {
    if principal_id == "owner" {
        return Some(V1Principal::Owner);
    }
    k2_core::api_keys::resolve_api_key_by_id(principal_id).map(V1Principal::Api)
}

// ── SQL helpers ──────────────────────────────────────────────────────

fn count_queued(workspace_path: &str) -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM host_session_spawn_queue \
         WHERE workspace_path = ?1 AND status = 'queued'",
        rusqlite::params![workspace_path],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .unwrap_or(0)
}

fn count_queued_global() -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM host_session_spawn_queue WHERE status = 'queued'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n as usize)
    .unwrap_or(0)
}

fn position_of(workspace_path: &str, job_id: &str) -> Option<usize> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    // FIFO by insert order: `rowid` is monotonic; `created_at` is only
    // second-granularity so same-second jobs would reorder by UUID string.
    let mut stmt = conn
        .prepare(
            "SELECT job_id FROM host_session_spawn_queue \
             WHERE workspace_path = ?1 AND status = 'queued' \
             ORDER BY rowid ASC",
        )
        .ok()?;
    let ids: Vec<String> = stmt
        .query_map(rusqlite::params![workspace_path], |r| r.get::<_, String>(0))
        .ok()?
        .flatten()
        .collect();
    ids.iter().position(|id| id == job_id).map(|i| i + 1)
}

fn find_open_by_client_request(
    workspace_path: &str,
    principal_id: &str,
    client_request_id: &str,
) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT job_id FROM host_session_spawn_queue \
         WHERE workspace_path = ?1 AND principal_id = ?2 \
           AND client_request_id = ?3 AND status IN ('queued','running') \
         ORDER BY created_at ASC LIMIT 1",
        rusqlite::params![workspace_path, principal_id, client_request_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

fn find_open_dead_resume(workspace_path: &str, session_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT job_id FROM host_session_spawn_queue \
         WHERE workspace_path = ?1 AND session_id = ?2 \
           AND kind = 'dead_resume' AND status IN ('queued','running') \
         ORDER BY created_at ASC LIMIT 1",
        rusqlite::params![workspace_path, session_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

fn peek_head(workspace_path: &str) -> Option<SpawnJob> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT job_id, workspace_path, workspace_slug, principal_id, kind, session_id, \
                prompt, timeout_secs, capabilities_json, cols, rows, model \
         FROM host_session_spawn_queue \
         WHERE workspace_path = ?1 AND status = 'queued' \
         ORDER BY rowid ASC LIMIT 1",
        rusqlite::params![workspace_path],
        |r| {
            let kind_s: String = r.get(4)?;
            let caps_s: Option<String> = r.get(8)?;
            let caps = caps_s.and_then(|s| serde_json::from_str(&s).ok());
            let timeout: Option<i64> = r.get(7)?;
            let cols: Option<i64> = r.get(9)?;
            let rows: Option<i64> = r.get(10)?;
            Ok(SpawnJob {
                job_id: r.get(0)?,
                workspace_path: r.get(1)?,
                workspace_slug: r.get(2)?,
                principal_id: r.get(3)?,
                kind: JobKind::parse(&kind_s).unwrap_or(JobKind::Cold),
                session_id: r.get(5)?,
                prompt: r.get(6)?,
                timeout_secs: timeout.map(|t| t as u64),
                capabilities: caps,
                cols: cols.map(|c| c as u16),
                rows: rows.map(|r| r as u16),
                model: r.get(11)?,
            })
        },
    )
    .ok()
}

/// Mark head `running`. Prompt is retained until terminal mark (spawn needs it;
/// the in-memory [`SpawnJob`] already holds a copy for the drain call).
fn claim_running(job_id: &str) -> bool {
    let now = now_secs();
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE host_session_spawn_queue \
         SET status = 'running', started_at = ?1, updated_at = ?1 \
         WHERE job_id = ?2 AND status = 'queued'",
        rusqlite::params![now, job_id],
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

fn mark_terminal(
    job_id: &str,
    status: JobStatus,
    result_session_id: Option<&str>,
    fail_code: Option<&str>,
    fail_message: Option<&str>,
) {
    debug_assert!(status.is_terminal());
    let now = now_secs();
    let db = k2_core::db::shared();
    let conn = db.lock();
    // Purge prompt on every terminal state (A8 / Q5).
    let _ = conn.execute(
        "UPDATE host_session_spawn_queue \
         SET status = ?1, updated_at = ?2, finished_at = ?2, \
             result_session_id = ?3, fail_code = ?4, fail_message = ?5, \
             prompt = NULL \
         WHERE job_id = ?6",
        rusqlite::params![
            status.as_str(),
            now,
            result_session_id,
            fail_code,
            fail_message,
            job_id,
        ],
    );
}

fn expire_stale_for_workspace(workspace_path: &str) {
    let cutoff = now_secs() - max_age_secs() as i64;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = conn.execute(
        "UPDATE host_session_spawn_queue \
         SET status = 'expired', updated_at = ?1, finished_at = ?1, prompt = NULL, \
             fail_code = 'expired', fail_message = 'max job age exceeded' \
         WHERE workspace_path = ?2 AND status = 'queued' AND created_at < ?3",
        rusqlite::params![now_secs(), workspace_path, cutoff],
    );
}

fn workspaces_with_queued() -> Vec<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = match conn.prepare(
        "SELECT DISTINCT workspace_path FROM host_session_spawn_queue \
         WHERE status = 'queued'",
    ) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    stmt.query_map([], |r| r.get::<_, String>(0))
        .ok()
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn load_job_view(workspace_path: &str, job_id: &str) -> Option<JobView> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = conn
        .query_row(
            "SELECT job_id, status, workspace_slug, workspace_path, principal_id, kind, \
                    session_id, client_request_id, created_at, updated_at, started_at, \
                    finished_at, result_session_id, fail_code, fail_message \
             FROM host_session_spawn_queue \
             WHERE job_id = ?1 AND workspace_path = ?2",
            rusqlite::params![job_id, workspace_path],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, Option<String>>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get::<_, i64>(8)?,
                    r.get::<_, i64>(9)?,
                    r.get::<_, Option<i64>>(10)?,
                    r.get::<_, Option<i64>>(11)?,
                    r.get::<_, Option<String>>(12)?,
                    r.get::<_, Option<String>>(13)?,
                    r.get::<_, Option<String>>(14)?,
                ))
            },
        )
        .ok()?;
    let status = JobStatus::parse(&row.1)?;
    let kind = JobKind::parse(&row.5)?;
    let mut view = JobView {
        job_id: row.0,
        status,
        workspace_slug: row.2,
        workspace_path: row.3,
        principal_id: row.4,
        kind,
        session_id: row.6,
        client_request_id: row.7,
        created_at: row.8,
        updated_at: row.9,
        started_at: row.10,
        finished_at: row.11,
        result_session_id: row.12,
        fail_code: row.13,
        fail_message: row.14,
        position: None,
    };
    drop(conn);
    if view.status == JobStatus::Queued {
        view.position = position_of(workspace_path, &view.job_id);
    }
    Some(view)
}

impl JobView {
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::json!({
            "jobId": self.job_id,
            "status": self.status.as_str(),
            "workspace": self.workspace_slug,
            "kind": self.kind.as_str(),
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        });
        if let Some(p) = self.position {
            v["position"] = serde_json::json!(p);
        }
        if let Some(ref sid) = self.session_id {
            v["sessionId"] = serde_json::json!(sid);
        }
        if let Some(ref sid) = self.result_session_id {
            v["sessionId"] = serde_json::json!(sid);
            v["resultSessionId"] = serde_json::json!(sid);
        }
        if let Some(t) = self.started_at {
            v["startedAt"] = serde_json::json!(t);
        }
        if let Some(t) = self.finished_at {
            v["finishedAt"] = serde_json::json!(t);
        }
        if let Some(ref c) = self.fail_code {
            v["code"] = serde_json::json!(c);
        }
        if let Some(ref m) = self.fail_message {
            v["error"] = serde_json::json!(m);
        }
        if let Some(ref cr) = self.client_request_id {
            v["clientRequestId"] = serde_json::json!(cr);
        }
        v
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_ws(tag: &str) -> (String, String) {
        let id = uuid::Uuid::new_v4();
        (
            format!("/tmp/k2-sq-{tag}-{id}"),
            format!("sq-{tag}-{}", &id.to_string()[..8]),
        )
    }

    fn enable_feature() {
        std::env::set_var("K2_HOST_SESSION_SPAWN_QUEUE", "1");
    }

    fn disable_feature() {
        std::env::remove_var("K2_HOST_SESSION_SPAWN_QUEUE");
    }

    fn base_req(ws_path: &str, slug: &str, principal: &str) -> EnqueueRequest {
        EnqueueRequest {
            workspace_path: ws_path.to_string(),
            workspace_slug: slug.to_string(),
            principal_id: principal.to_string(),
            kind: JobKind::Cold,
            session_id: None,
            prompt: Some("secret-prompt-do-not-log".into()),
            timeout_secs: Some(180),
            capabilities: None,
            cols: None,
            rows: None,
            client_request_id: None,
            model: None,
        }
    }

    /// Ensure migration-created table exists (shared test DB).
    fn ensure_table() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        // Migrations run at open; probe the table.
        let ok: bool = conn
            .query_row(
                "SELECT COUNT(*) >= 0 FROM host_session_spawn_queue",
                [],
                |r| r.get::<_, i64>(0).map(|n| n >= 0),
            )
            .unwrap_or(false);
        assert!(ok, "host_session_spawn_queue table must exist (migration 0096)");
    }

    #[test]
    fn feature_default_off() {
        // May be set by parallel tests — pin OFF for this assertion.
        let prev = std::env::var("K2_HOST_SESSION_SPAWN_QUEUE").ok();
        std::env::remove_var("K2_HOST_SESSION_SPAWN_QUEUE");
        assert!(!feature_enabled(), "default must be OFF");
        if let Some(v) = prev {
            std::env::set_var("K2_HOST_SESSION_SPAWN_QUEUE", v);
        }
    }

    #[test]
    fn enqueue_assigns_fifo_positions() {
        ensure_table();
        enable_feature();
        let (path, slug) = unique_ws("fifo");
        let p = "p-fifo";
        let (j1, pos1) = enqueue(base_req(&path, &slug, p)).expect("e1");
        let (j2, pos2) = enqueue(base_req(&path, &slug, p)).expect("e2");
        let (j3, pos3) = enqueue(base_req(&path, &slug, p)).expect("e3");
        assert_eq!(pos1, 1);
        assert_eq!(pos2, 2);
        assert_eq!(pos3, 3);
        assert_ne!(j1, j2);
        assert_eq!(position_of(&path, &j1), Some(1));
        assert_eq!(position_of(&path, &j3), Some(3));
        // Cleanup
        for j in [&j1, &j2, &j3] {
            mark_terminal(j, JobStatus::Cancelled, None, None, None);
        }
        disable_feature();
    }

    #[test]
    fn enqueue_persists_model_through_peek_head() {
        ensure_table();
        enable_feature();
        let (path, slug) = unique_ws("model");
        let mut req = base_req(&path, &slug, "p-model");
        req.model = Some("sonnet".into());
        let (job_id, _) = enqueue(req).expect("enqueue with model");
        let head = peek_head(&path).expect("queued head");
        assert_eq!(head.job_id, job_id);
        assert_eq!(
            head.model.as_deref(),
            Some("sonnet"),
            "drain reconstruction must keep API model"
        );
        mark_terminal(&job_id, JobStatus::Cancelled, None, None, None);
        disable_feature();
    }

    #[test]
    fn depth_full_returns_spawn_queue_full() {
        ensure_table();
        enable_feature();
        std::env::set_var("K2_HOST_SESSION_QUEUE_DEPTH", "2");
        let (path, slug) = unique_ws("depth");
        let p = "p-depth";
        enqueue(base_req(&path, &slug, p)).expect("1");
        enqueue(base_req(&path, &slug, p)).expect("2");
        let err = enqueue(base_req(&path, &slug, p)).expect_err("3 must fail");
        assert!(matches!(err, EnqueueError::DepthFull));
        let resp = queue_full_response();
        assert_eq!(resp.status, "429 Too Many Requests");
        assert!(resp.body.contains("spawn-queue-full"));
        std::env::remove_var("K2_HOST_SESSION_QUEUE_DEPTH");
        disable_feature();
    }

    #[test]
    fn cancel_removes_job_and_updates_positions() {
        ensure_table();
        enable_feature();
        let (path, slug) = unique_ws("cancel");
        let p = "p-cancel";
        let (j1, _) = enqueue(base_req(&path, &slug, p)).unwrap();
        let (j2, _) = enqueue(base_req(&path, &slug, p)).unwrap();
        let view = cancel_job(&path, &j1, p).expect("cancel head");
        assert_eq!(view.status, JobStatus::Cancelled);
        // j2 is now position 1.
        assert_eq!(position_of(&path, &j2), Some(1));
        // Prompt purged.
        let db = k2_core::db::shared();
        let conn = db.lock();
        let prompt: Option<String> = conn
            .query_row(
                "SELECT prompt FROM host_session_spawn_queue WHERE job_id = ?1",
                rusqlite::params![j1],
                |r| r.get(0),
            )
            .unwrap();
        assert!(prompt.is_none() || prompt.as_deref() == Some(""), "prompt purged");
        mark_terminal(&j2, JobStatus::Cancelled, None, None, None);
        disable_feature();
    }

    #[test]
    fn client_request_id_is_idempotent() {
        ensure_table();
        enable_feature();
        let (path, slug) = unique_ws("idem");
        let p = "p-idem";
        let mut r = base_req(&path, &slug, p);
        r.client_request_id = Some("crid-1".into());
        let (j1, _) = enqueue(r.clone()).unwrap();
        let (j2, _) = enqueue(r).unwrap();
        assert_eq!(j1, j2, "same clientRequestId → same jobId");
        mark_terminal(&j1, JobStatus::Cancelled, None, None, None);
        disable_feature();
    }

    #[test]
    fn dead_resume_duplicate_returns_existing() {
        ensure_table();
        enable_feature();
        let (path, slug) = unique_ws("dres");
        let p = "p-dres";
        let sid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let mut r = base_req(&path, &slug, p);
        r.kind = JobKind::DeadResume;
        r.session_id = Some(sid.into());
        let (j1, _) = enqueue(r.clone()).unwrap();
        let err = enqueue(r).expect_err("duplicate");
        match err {
            EnqueueError::DeadResumeDuplicate { job_id, .. } => assert_eq!(job_id, j1),
            _ => panic!("expected DeadResumeDuplicate"),
        }
        mark_terminal(&j1, JobStatus::Cancelled, None, None, None);
        disable_feature();
    }

    #[test]
    fn release_wakes_fifo_head_with_mock_spawn() {
        ensure_table();
        enable_feature();
        sandbox_quota::test_reset_all();
        let (path, slug) = unique_ws("wake");
        // "owner" reloads without an api_keys row (drain principal reload).
        let p = "owner";

        // Hold one slot so admit-path would be at cap (ws_cap=1 for fill).
        let held = sandbox_quota::try_acquire_in_workspace_with_cap_nowait(p, Some(&path), 1);
        assert!(held.is_ok());

        let (j1, _) = enqueue(base_req(&path, &slug, p)).unwrap();
        let (j2, _) = enqueue(base_req(&path, &slug, p)).unwrap();

        let order = std::sync::Arc::new(Mutex::new(Vec::new()));
        {
            let order_c = order.clone();
            let mut hook = TEST_SPAWN_HOOK.lock().unwrap();
            *hook = Some(Box::new(move |job: &SpawnJob| {
                order_c.lock().unwrap().push(job.job_id.clone());
                Ok(format!("sess-{}", job.job_id))
            }));
        }

        // Free the fill slot → drain runs. Mock spawn keeps acquired slots,
        // so with default workspace cap (15) both jobs may complete in one
        // wake; FIFO order is what we pin.
        sandbox_quota::release_in_workspace(p, Some(&path));

        let v1 = get_job(&path, &j1).expect("j1");
        assert_eq!(
            v1.status,
            JobStatus::Completed,
            "FIFO head must complete on release; got {:?}",
            v1.status
        );
        let expected = format!("sess-{j1}");
        assert_eq!(
            v1.result_session_id.as_deref(),
            Some(expected.as_str()),
            "result sessionId assigned at completed"
        );

        // Ensure j2 also eventually completes (second release if still queued).
        let v2 = get_job(&path, &j2).expect("j2");
        if v2.status == JobStatus::Queued {
            sandbox_quota::release_in_workspace(p, Some(&path));
        }
        let v2 = get_job(&path, &j2).expect("j2 after drain");
        assert_eq!(v2.status, JobStatus::Completed, "second job must complete");

        let ord = order.lock().unwrap().clone();
        assert!(
            ord.len() >= 2,
            "both jobs must spawn; order={ord:?}"
        );
        assert_eq!(ord[0], j1, "strict FIFO: head first");
        assert_eq!(ord[1], j2, "strict FIFO: second next");

        *TEST_SPAWN_HOOK.lock().unwrap() = None;
        sandbox_quota::test_reset_all();
        disable_feature();
    }

    #[test]
    fn principal_gone_fails_job_and_continues() {
        ensure_table();
        enable_feature();
        sandbox_quota::test_reset_all();
        let (path, slug) = unique_ws("pgone");
        // Principal id that is NOT owner and NOT in api_keys.
        let ghost = format!("ghost-{}", uuid::Uuid::new_v4());
        let (j1, _) = enqueue(base_req(&path, &slug, &ghost)).unwrap();
        // Free a slot so drain runs (principal check is before acquire).
        on_slot_freed(None, Some(&path));
        let v = get_job(&path, &j1).expect("job");
        assert_eq!(v.status, JobStatus::Failed);
        assert_eq!(v.fail_code.as_deref(), Some("principal-gone"));
        disable_feature();
    }

    #[test]
    fn prompt_not_in_job_view_json() {
        ensure_table();
        enable_feature();
        let (path, slug) = unique_ws("prompt");
        let (j, _) = enqueue(base_req(&path, &slug, "p")).unwrap();
        let view = get_job(&path, &j).unwrap();
        let json = view.to_json().to_string();
        assert!(
            !json.contains("secret-prompt"),
            "prompt must never appear in job status JSON"
        );
        mark_terminal(&j, JobStatus::Cancelled, None, None, None);
        disable_feature();
    }
}
