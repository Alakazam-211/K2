//! F1 (prd-v1-api-completion §3) — NON-SANDBOXED **HOST SESSIONS** on `/v1`:
//! the sibling route family that makes a sandbox-less host (K2 Cloud
//! Standard, shared VPS, Raspberry Pi, any Mac) a first-class API citizen.
//!
//! ```text
//! POST /v1/w/<ws>/host-sessions              → spawn (or resume with {"session": id})
//! GET  /v1/w/<ws>/host-sessions              → list this ws's api-spawned host sessions
//! GET  /v1/w/<ws>/host-sessions/queue        → list open spawn-queue jobs (feature)
//! GET  /v1/w/<ws>/host-sessions/queue/<id>   → job status
//! POST /v1/w/<ws>/host-sessions/queue/<id>/cancel → cancel a queued job
//! POST /v1/w/<ws>/host-sessions/<id>         → message-live (inject into the PTY)
//! GET  /v1/w/<ws>/host-sessions/<id>         → status (started/live/phase — Scout reconciler)
//! GET  /v1/w/<ws>/host-sessions/<id>/messages?since=<seq>
//! POST /v1/w/<ws>/host-sessions/<id>/kill    → force-stop the live PTY (integrator spend-cap)
//! ```
//!
//! GATING: `misc_routes::api_enabled()` ONLY (`K2_API`, or the legacy
//! `K2_SANDBOX_API` which implies it) — deliberately NOT the sandbox-family
//! gate. This family is available on EVERY host, including sandbox-capable
//! ones (Dedicated gets both doors); `can_sandbox()` is irrelevant here.
//!
//! HONESTY INVARIANT (PRD §2): this is a DIFFERENT, honestly-labeled door
//! from `/v1/sandboxes` — every spawn response carries `"sandbox":"none"`.
//! The sandbox routes keep their 409-refusal; nothing here ever pretends
//! isolation it doesn't have.
//!
//! AUTHZ (identical discipline to `v1_sandboxes`):
//! - `v1_principal` resolved by the dispatcher (owner token or API key);
//! - per-key workspace grant via [`crate::v1_sandboxes::resolve_authorized_workspace`]
//!   (fail-closed NULL grant);
//! - uniform 404 for unknown ws / ungranted ws / unknown-or-unowned session
//!   alike — never an existence oracle;
//! - CANONICAL OFF-LIMITS: the workspace's pinned canonical session is never
//!   spawnable/addressable/readable here ([`crate::v1_sandboxes::session_is_canonical`]);
//!   the canonical agent stays reachable only via `POST /v1/w/<ws>/message`
//!   with its consent + busy gates.
//!
//! The security-critical passthrough policy resolver lives in
//! [`policy::resolve_host_spawn`]; quota ([`crate::sandbox_quota`]) and the
//! idle reaper ([`crate::sandbox_reaper`]) are reused as-is (both are
//! principal/session keyed, not microVM-specific).
//!
//! SESSION-IDENTITY COVERAGE (Slice W3 — honest limits):
//! - **Premint providers (claude, grok)** — the host splices
//!   `--session-id <sid>` at spawn; the id is durable immediately, so
//!   list/resume work from the first second.
//! - **Self-minting adapter providers (codex, gemini, pi, cursor, hermes)**
//!   — the provider mints its own conversation id after boot; the spawn argv
//!   carries none, so the row starts `session_id = NULL`. A deferred
//!   adoption probe ([`defer_adopt_api_host_session`], the wake path's
//!   `defer_stamp_adopted_session` seam re-aimed at the `api-…` tab row)
//!   discovers the provider-minted id from the provider's own on-disk store
//!   and stamps it — list shows the session and resume matches once the
//!   agent has booted and persisted its first record (~seconds).
//! - **Presets with NO adapter (opencode, goose, aider, ollama, copilot,
//!   interpreter)** — K2 has no session-store adapter to discover an id
//!   with, so their rows stay `session_id = NULL` forever: they spawn and
//!   accept live messages fine, but are DELIBERATELY ABSENT from
//!   `GET …/host-sessions` and can never be resumed. That absence is the
//!   honest answer, not a bug — listing an unresumable id would be a lie.

pub mod policy;

use std::time::Duration;

use crate::cli_response::CliResponse;
use crate::routes::http::{V1Capability, V1Principal};
use crate::v1_sandboxes::{
    decode_and_validate_segment, resolve_authorized_workspace, session_is_canonical,
    uniform_ws_404,
};
use crate::{sandbox_quota, stream_token, v2_spawn};

use k2_core::log_debug;
use k2_core::session::SessionId;

use policy::ApiHostSessionRequest;

/// How long the post-spawn prompt injector waits for the fresh agent TUI to
/// come ready before injecting best-effort (background thread — never blocks
/// the API response). Env-overridable (`K2_HOST_SESSION_READY_TIMEOUT_SECS`)
/// so the integration suite can clamp the ceiling for shim agents that never
/// advertise readiness — mirrors the wake path's
/// [`crate::workspace_msg::DEFAULT_WAKE_TIMEOUT`] (20s).
const SPAWN_PROMPT_READY_TIMEOUT_SECS: u64 = 20;

/// Follow-up inject into an already-live host session: same readiness shape
/// as post-wake (`deliver_post_wake` via `inject_raw_…_with_profile`) but a
/// shorter ceiling so an idle composer still settles quickly (paste already
/// on → skip the post-flip 1s, then quiescence) without blocking chat for
/// a full cold-start window.
const HOST_FOLLOWUP_INJECT_TIMEOUT_SECS: u64 = 10;

fn spawn_prompt_ready_timeout() -> Duration {
    let secs = std::env::var("K2_HOST_SESSION_READY_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(SPAWN_PROMPT_READY_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// FROZEN contract preamble (byte-stable — tests pin it) prepended to the
/// INITIAL prompt injected into a freshly spawned host-session process. It
/// is the only place the in-session agent is ever TOLD the `k2 respond`
/// read-back contract, so it goes to every process that has never seen it:
/// a fresh spawn AND a resume-of-a-DEAD-session (which re-spawns a new
/// process below — new process, never saw the contract). It deliberately
/// does NOT go to follow-up deliveries into a live session
/// ([`deliver_into_live`]: message-live and resume-of-live) — the contract
/// was already delivered in-session and repeating it would pollute every
/// turn.
///
/// Injection stack (Phase 0b):
/// ```text
/// [Platform] API_SPAWN_PREAMBLE     — spawn-only (this const)
/// [Owner]    api_guest_policy      — every turn (spawn + message-live)
/// [Caller]   prompt                — untrusted
/// ```
pub const API_SPAWN_PREAMBLE: &str = "[K2 API] You were invoked through the K2 API. Report progress and results back to the caller by running: k2 respond '<your message>' — and send your final answer with: k2 respond --final '<your answer>'. The caller cannot see your terminal; only k2 respond output reaches them.";

/// Compose the spawn-time inject payload: frozen preamble, then owner guest
/// policy, then the untrusted caller prompt. Pure — host-resolved policy
/// only; never takes a body field for the middle layer.
pub fn compose_spawn_inject(guest_policy: &str, caller_prompt: &str) -> String {
    format!("{API_SPAWN_PREAMBLE}\n\n{guest_policy}\n\n{caller_prompt}")
}

/// Compose a follow-up (message-live / resume-of-live) inject payload:
/// re-assert owner guest policy, then the untrusted caller prompt. Does
/// NOT re-send [`API_SPAWN_PREAMBLE`] (already briefed once per process).
pub fn compose_followup_inject(guest_policy: &str, caller_prompt: &str) -> String {
    format!("{guest_policy}\n\n{caller_prompt}")
}

/// How long the deferred adoption probe waits before reading the provider's
/// on-disk session store — same ~5s window as the wake path's
/// `defer_stamp_adopted_session` (providers persist their session record a
/// beat after spawn). Env-overridable (`K2_HOST_SESSION_ADOPTION_DELAY_MS`)
/// so the integration suite's shim scenario doesn't sleep 5 real seconds.
const HOST_ADOPTION_PROBE_DELAY_MS: u64 = 5000;

fn host_adoption_probe_delay() -> Duration {
    let ms = std::env::var("K2_HOST_SESSION_ADOPTION_DELAY_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(HOST_ADOPTION_PROBE_DELAY_MS);
    Duration::from_millis(ms)
}

/// Deferred post-spawn adoption for SELF-MINTING providers (Slice W3): the
/// host-sessions sibling of `wake_headless::defer_stamp_adopted_session`.
/// Sleep the probe window on a detached thread, discover the id the freshly
/// spawned provider actually minted on disk
/// ([`ProviderResume::newest_on_disk`] — the same probe the wake path
/// trusts), then stamp it onto THIS spawn's `api-…` row in
/// `workspace_tab_sessions` — NOT onto `workspace_sessions` like the wake
/// path does: that row is the CANONICAL pinned chat's identity SSOT and is
/// off-limits to this API family by design.
///
/// Once stamped, `GET …/host-sessions` lists the session and
/// `{"session": <id>}` resumes it. Fire-and-forget: a miss (agent still
/// booting at probe time, provider wrote nothing) just leaves the row NULL —
/// the documented degraded state, never an error.
fn defer_adopt_api_host_session(
    provider: &'static str,
    ws_path: String,
    agent_name: String,
    principal_key: String,
) {
    std::thread::spawn(move || {
        std::thread::sleep(host_adoption_probe_delay());
        match adopt_api_host_session(provider, &ws_path, &agent_name, &principal_key) {
            Some(sid) => log_debug!(
                "[v1-host] adopted {} session {} for api agent {} in {}",
                provider,
                sid,
                agent_name,
                ws_path
            ),
            None => log_debug!(
                "[v1-host] deferred {} adoption for api agent {} found nothing to stamp \
                 (no on-disk session yet, canonical-only, or row already stamped) in {}",
                provider,
                agent_name,
                ws_path
            ),
        }
    });
}

/// Synchronous core of [`defer_adopt_api_host_session`]. Returns the adopted
/// id, or `None` when there is nothing safe to stamp:
/// - the provider wrote no on-disk session for this workspace yet;
/// - the newest on-disk id is the workspace's CANONICAL session
///   (`newest_on_disk` is an approximation — if OUR spawn hasn't persisted
///   yet, the newest conversation may be the pinned chat's; adopting it would
///   hand this API family a canonical handle, so refuse. The resume door's
///   own canonical guard is the backstop either way);
/// - the row is already stamped (never clobber — first truth wins).
///
/// On success, also records the spawning principal as the OWNER of the
/// provider-minted id ([`crate::sandbox_responses::record_owner`]) so
/// message-live / messages addressed by the ADOPTED id pass the same
/// default-deny gate as the spawn-returned id.
fn adopt_api_host_session(
    provider: &str,
    ws_path: &str,
    agent_name: &str,
    principal_key: &str,
) -> Option<String> {
    let adapter =
        k2_core::workspace::provider_resume::provider_resume_for_provider(provider)?;
    let sid = adapter.newest_on_disk(ws_path)?;
    if sid.is_empty() || session_is_canonical(&sid) {
        return None;
    }
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let project_id =
            k2_core::workspace::agent_identity::resolve_project_id(&conn, ws_path)?;
        let row = k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(
            &conn, &project_id, agent_name,
        )
        .ok()
        .flatten()?;
        if row.session_id.is_some() {
            return None; // already stamped — never clobber.
        }
        k2_core::db::schema::WorkspaceTabSession::stamp_session_id(
            &conn,
            &project_id,
            &row.pane_group_id,
            &sid,
        )
        .ok()?;
    }
    crate::sandbox_responses::record_owner(&sid, principal_key);
    Some(sid)
}

/// Resolve a LIVE PTY for a host session addressed by `session_id` within
/// `ws_path`. Two shapes exist (Slice W3):
/// - premint providers — the daemon SessionId IS the conversation id, so the
///   direct `v2_session_map` lookup hits;
/// - self-minting providers — the conversation id was ADOPTED post-hoc and
///   differs from the live PTY's forced daemon SessionId, so map through the
///   `api-…` tab row's `agent_name` (the v2 map key) instead.
///
/// **Liveness honesty (launch-param PRD D2 / A11):** a map hit is LIVE only
/// when [`DaemonPtySession::is_child_alive`] is true. Phantom rows (dead
/// child still mapped) are treated as DEAD so the resume door falls through
/// to dead-resume + launch-param. Prefers [`reconcile_dead_children`] so
/// zombies do not steal LIVE.
///
/// Scoped to THIS workspace's `api-…` rows on the adopted path (same index
/// as [`host_session_resumable`]); the direct path's workspace pin is
/// enforced by the callers' existing cwd checks / resume-door index check.
fn lookup_live_host_session(
    ws_path: &str,
    session_id: &str,
) -> Option<std::sync::Arc<k2_core::terminal::DaemonPtySession>> {
    // Drop map entries whose child already exited but never unregistered
    // so phantom rows don't steal the LIVE arm (list already does this).
    crate::v2_session_map::reconcile_dead_children();

    if let Some(sid) = SessionId::parse(session_id) {
        if let Some(live) = crate::v2_session_map::lookup_by_session_id(&sid) {
            if live.is_child_alive() {
                return Some(live);
            }
            // Phantom: map present, child dead → treat as DEAD.
            return None;
        }
    }
    let agents: Vec<String> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT wts.agent_name FROM workspace_tab_sessions wts \
                 JOIN projects p ON p.id = wts.project_id \
                 WHERE p.path = ?1 AND wts.session_id = ?2 \
                   AND wts.agent_name LIKE 'api-%' \
                 ORDER BY wts.last_seen_at DESC",
            )
            .ok()?;
        let mapped = stmt
            .query_map(rusqlite::params![ws_path, session_id], |r| {
                r.get::<_, String>(0)
            })
            .ok()?;
        mapped.flatten().collect()
    };
    agents.iter().find_map(|a| {
        let live = crate::v2_session_map::lookup_by_agent_name(a)?;
        live.is_child_alive().then_some(live)
    })
}

/// Parse the UNTRUSTED body into hints. Empty/whitespace → all defaults;
/// malformed JSON → `Err(400)` (mirror of the sandbox doors).
fn parse_body(body: &[u8]) -> Result<ApiHostSessionRequest, CliResponse> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(ApiHostSessionRequest::default());
    }
    serde_json::from_slice(body)
        .map_err(|e| CliResponse::bad_request(format!("invalid JSON body: {e}")))
}

/// Is `session_id` a resumable HOST session of the workspace at `ws_path`?
///
/// The durable per-workspace index is `workspace_tab_sessions` (stamped by
/// `v2_session_map::register` from the host-spliced `--session-id`/`--resume`
/// argv), restricted to the `api-…` agent namespace — so a caller can resume
/// ONLY sessions this API family itself spawned in THIS workspace: never a
/// human tab's transcript, never another workspace's, never the canonical
/// session (that id lives in `workspace_sessions`, refused separately by the
/// canonical guard). FAIL-CLOSED on DB error.
fn host_session_resumable(ws_path: &str, session_id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT COUNT(*) FROM workspace_tab_sessions wts \
         JOIN projects p ON p.id = wts.project_id \
         WHERE p.path = ?1 AND wts.session_id = ?2 AND wts.agent_name LIKE 'api-%'",
        rusqlite::params![ws_path, session_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Shared live-delivery step for message-live and resume-of-a-live-session:
/// re-arm the idle reaper and inject the (optional) prompt through the
/// locked, sanitized injector. Returns the FROZEN live-delivery response
/// `{sessionId, delivered, live:true}`.
///
/// `sid` is the live PTY's daemon SessionId (reaper key + injector target);
/// `echo_sid` is the id the CALLER addressed — identical for premint
/// providers, the ADOPTED provider-minted id for self-minting ones (echoing
/// the daemon-internal id there would hand back a handle no other route
/// resolves).
///
/// Phase 0b: when `prompt` is non-empty, the workspace's owner guest
/// policy is prepended ([`compose_followup_inject`]) — never taken from
/// the request body. Empty prompt = reaper re-arm only (no inject).
///
/// Inject shape matches `k2 msg` post-wake ([`workspace_msg::deliver_post_wake`]
/// via profile-aware inject): poll `?2004h` from t=0 (1s after a 0→1 flip)
/// or liar settle → screen quiescence → paste. `wait_ready = 0` was a
/// mid-turn black hole (paste into a still-streaming Grok/Claude frame).
/// Live inject / live-resume. Optionally re-mints capability JWTs to the
/// session cap **file** when `capabilities` is present (process env is
/// immutable — file is multi-turn SSOT; agent re-reads each turn).
fn deliver_into_live(
    sid: &SessionId,
    echo_sid: &str,
    prompt: &str,
    ws_path: &str,
    slug: &str,
    principal_key: &str,
    capabilities: Option<&serde_json::Value>,
    timeout_secs: Option<u64>,
    model: Option<&str>,
) -> CliResponse {
    crate::sandbox_reaper::stamp(sid);

    // Live resume remint (0.40.76 / PRD): caps present → fresh JWTs + atomic
    // file overwrite; omit → no remint. Never mutates live process env.
    // Prior jtis stay valid until their exp (Scout-local revoke if single-valid).
    let mut caps_meta: Option<serde_json::Value> = None;
    if let Some(caps_val) = capabilities {
        let exp_secs = crate::sandbox_reaper::normalize_timeout(timeout_secs)
            .min(3600)
            .max(1);
        match crate::v1_capabilities::parse_capabilities(caps_val).and_then(|caps| {
            crate::v1_capabilities::mint_and_stage(
                echo_sid,
                std::path::Path::new(ws_path),
                &caps,
                exp_secs,
                Some(principal_key),
                Some(slug),
            )
        }) {
            Ok(out) => caps_meta = Some(out.metadata),
            Err(e) => {
                return CliResponse {
                    status: "400 Bad Request",
                    content_type: "application/json",
                    body: serde_json::json!({ "error": e, "code": "capabilities-invalid" })
                        .to_string(),
                };
            }
        }
    }

    let delivered = if prompt.is_empty() {
        // Nothing to inject — the touch (reaper re-arm) is the whole effect.
        true
    } else {
        let guest = k2_core::workspace::settings::get_api_guest_policy(ws_path);
        let payload = compose_followup_inject(&guest, prompt);
        let inject_profile = policy::resolve_host_injection_profile(ws_path);
        let ok = crate::workspace_msg::inject_raw_into_session_with_profile(
            sid,
            &payload,
            Duration::from_secs(HOST_FOLLOWUP_INJECT_TIMEOUT_SECS),
            &inject_profile,
        );
        if !ok {
            log_debug!(
                "[v1-host] follow-up inject session={} delivered=false (pty gone, gate, or wait failed)",
                sid
            );
        }
        ok
    };
    // S4 R1: live inject is always resumed:true (same PTY).
    // F4: `workspace` is ALWAYS the URL slug (same as cold-spawn / dead-resume),
    // never the filesystem path — integrators key plan→session registries on it.
    let mut body = serde_json::json!({
        "sessionId": echo_sid,
        "delivered": delivered,
        "live": true,
        "resumed": true,
        "workspace": slug,
        "sandbox": "none",
    });
    if let Some(meta) = caps_meta {
        body["capabilities"] = meta;
    }
    if model.map(str::trim).filter(|s| !s.is_empty()).is_some() {
        body["modelSource"] = serde_json::json!("ignored_live");
        body["modelIgnored"] = serde_json::json!("live_session");
        body["modelApplied"] = serde_json::Value::Null;
    }
    CliResponse::ok_json(body.to_string())
}

/// Phase 0 capability gate for the host-sessions family. Missing cap → the
/// same uniform workspace 404 as an ungranted key (no existence oracle).
fn require_host_sessions(principal: &V1Principal) -> Result<(), CliResponse> {
    if principal.has_capability(V1Capability::HostSessions) {
        Ok(())
    } else {
        Err(uniform_ws_404())
    }
}

/// `POST /v1/w/<ws>/host-sessions` — spawn a fresh NON-SANDBOXED host session
/// in the granted workspace (or RESUME one of this family's own prior
/// sessions when the body carries `{"session": <id>}`).
///
/// Cap admit (prd-host-session-spawn-queue-v1):
/// - **Live hit** → inject (no queue, no acquire).
/// - **Feature OFF** → full S8 wait then 429 (byte-compatible pre-PRD).
/// - **Feature ON** → always nowait acquire; refuse → enqueue (or immediate
///   429 when `queue:false` / depth full). Never long S8 while feature ON.
pub(crate) fn handle_v1_host_new(principal: &V1Principal, ws_raw: &str, body: &[u8]) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let req = match parse_body(body) {
        Ok(r) => r,
        Err(resp) => return resp,
    };

    // Resume intent: `{"session": <id>}`. A present-but-malformed, canonical,
    // or not-this-family's id is a hard uniform 404 (the caller asked for
    // specific state; never silently downgrade to a fresh session). The
    // target is kept as a STRING: for self-minting providers the indexed
    // (adopted) conversation id is provider-minted and — for hermes — not
    // UUID-shaped at all; the index check against this workspace's own
    // `api-…` rows is the authorization, not the id's shape.
    let resume_target: Option<String> = match req.session.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let Some(validated) = decode_and_validate_segment(raw) else {
                return uniform_ws_404();
            };
            if session_is_canonical(&validated) {
                return uniform_ws_404();
            }
            if !host_session_resumable(&ws_path, &validated) {
                return uniform_ws_404();
            }
            Some(validated)
        }
    };

    // Resume of a session that is STILL LIVE = deliver into it (never boot a
    // duplicate PTY on the same conversation) — the F3 liveness-router shape.
    // Live inject never queues (A7 / product lock 10).
    if let Some(target) = resume_target.as_deref() {
        if let Some(live) = lookup_live_host_session(&ws_path, target) {
            let prompt = req.prompt.as_deref().unwrap_or("").trim().to_string();
            return deliver_into_live(
                &live.session_id,
                target,
                &prompt,
                &ws_path,
                &slug,
                &principal.display_id(),
                req.capabilities.as_ref(),
                req.timeout_secs,
                req.model.as_deref(),
            );
        }
    }

    let principal_key = principal.display_id();
    let ws_cell_cap =
        k2_core::workspace::settings::get_host_session_cell_cap(&ws_path);
    let is_resume = resume_target.is_some();
    let queue_feature = crate::spawn_queue::feature_enabled();

    // Cap admit.
    if queue_feature {
        // Feature ON: always nowait (Q1 / A12 fairness).
        if let Err(qe) = sandbox_quota::try_acquire_in_workspace_with_cap_nowait(
            &principal_key,
            Some(&ws_path),
            ws_cell_cap,
        ) {
            // Explicit queue:false → immediate 429 (no S8, no enqueue).
            let queue_allowed = req.queue.unwrap_or(true);
            if !queue_allowed {
                return CliResponse {
                    status: "429 Too Many Requests",
                    content_type: "application/json",
                    body: serde_json::json!({ "error": qe.message(), "code": qe.code() })
                        .to_string(),
                };
            }
            // Enqueue cold or dead_resume.
            let kind = if is_resume {
                crate::spawn_queue::JobKind::DeadResume
            } else {
                crate::spawn_queue::JobKind::Cold
            };
            let enq = crate::spawn_queue::EnqueueRequest {
                workspace_path: ws_path.clone(),
                workspace_slug: slug.clone(),
                principal_id: principal_key.clone(),
                kind,
                session_id: resume_target.clone(),
                // Sole durable prompt copy — never logged.
                prompt: req.prompt.clone(),
                timeout_secs: req.timeout_secs,
                // Specs only — JWTs minted at drain.
                capabilities: req.capabilities.clone(),
                cols: req.cols,
                rows: req.rows,
                client_request_id: req
                    .client_request_id
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
                model: req
                    .model
                    .as_ref()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            };
            return match crate::spawn_queue::enqueue(enq) {
                Ok((job_id, position)) => crate::spawn_queue::queued_response(
                    &job_id,
                    position,
                    &slug,
                    resume_target.as_deref(),
                ),
                Err(crate::spawn_queue::EnqueueError::DepthFull) => {
                    crate::spawn_queue::queue_full_response()
                }
                Err(crate::spawn_queue::EnqueueError::DeadResumeDuplicate {
                    job_id,
                    position,
                }) => {
                    // Second POST for same (ws, session) → existing job (or 409).
                    // Return 202 with the existing jobId (idempotent shape).
                    crate::spawn_queue::queued_response(
                        &job_id,
                        position,
                        &slug,
                        resume_target.as_deref(),
                    )
                }
            };
        }
    } else {
        // Feature OFF: legacy full S8 wait then 429 only.
        if let Err(qe) = sandbox_quota::try_acquire_in_workspace_with_cap(
            &principal_key,
            Some(&ws_path),
            ws_cell_cap,
        ) {
            return CliResponse {
                status: "429 Too Many Requests",
                content_type: "application/json",
                body: serde_json::json!({ "error": qe.message(), "code": qe.code() }).to_string(),
            };
        }
    }

    // Slot held → shared post-acquire spawn (also used by queue drain).
    spawn_host_session_after_acquire(
        principal,
        &ws_path,
        &slug,
        &req,
        is_resume,
        resume_target.as_deref(),
    )
}

/// Post-quota-acquire spawn path shared by admit and the durable queue drain.
///
/// **Must not** re-acquire quota. Caller already holds principal + workspace +
/// global slots. On early failure this function releases; on successful spawn
/// the child-exit observer owns the release.
///
/// Mint capability JWTs here (drain-time for queued jobs) — never at enqueue.
/// `status: completed` for queue jobs means this returned 200 + sessionId
/// (cell started), not agent `--final`.
pub(crate) fn spawn_host_session_after_acquire(
    principal: &V1Principal,
    ws_path: &str,
    slug: &str,
    req: &ApiHostSessionRequest,
    is_resume: bool,
    resume_target: Option<&str>,
) -> CliResponse {
    let principal_key = principal.display_id();

    // The forced daemon SessionId: a UUID-shaped resume target rides it
    // (premint continuity — returned id == conversation id); a non-UUID
    // provider-minted target (hermes) can't, so the PTY gets a fresh
    // daemon id while the RESUME GRAMMAR below still carries the target
    // (policy splices `req.session`, and registration re-stamps the new
    // `api-…` row with it via the table-driven argv scan).
    let session_id = resume_target
        .and_then(SessionId::parse)
        .unwrap_or_else(SessionId::new);

    // The PASSTHROUGH POLICY RESOLVER (the one new security-critical piece):
    // cwd pinned, command host-minted from the workspace's configured agent,
    // caller env/args dropped, principal key staged, danger flags stripped
    // unless the owner opted in.
    let (mut spawn_req, launch_plan, model_decision) =
        policy::resolve_host_spawn(principal, ws_path, &session_id, is_resume, req);
    spawn_req.principal_key = Some(principal_key.clone());
    spawn_req.quota_workspace = Some(ws_path.to_string());
    // Captured for the post-spawn adoption decision below (spawn_session
    // consumes the request).
    let spawn_command = spawn_req.command.clone().unwrap_or_default();

    // Idle-reap timeout — the caller's knob, identical semantics to the
    // sandbox family (clamped 30..86400, default 180).
    let timeout_secs = crate::sandbox_reaper::normalize_timeout(req.timeout_secs);

    // S1–S3: mint capability JWTs into host-curated env BEFORE spawn so the
    // agent process sees K2_CAPABILITY_TOKEN at start (never in free-form prompt).
    // Drain path mints HERE — enqueue stored specs only (Q5 / A13).
    let mut caps_meta: Option<serde_json::Value> = None;
    if let Some(caps_val) = req.capabilities.as_ref() {
        let sid_str = session_id.to_string();
        let exp_secs = timeout_secs.min(3600).max(1);
        match crate::v1_capabilities::parse_capabilities(caps_val).and_then(|caps| {
            crate::v1_capabilities::mint_and_stage(
                &sid_str,
                std::path::Path::new(ws_path),
                &caps,
                exp_secs,
                Some(principal_key.as_str()),
                Some(slug),
            )
        }) {
            Ok(out) => {
                if !out.env_value.is_empty() && out.env_value != "[]" {
                    if let Some(env) = spawn_req.env.as_mut() {
                        env.insert(
                            crate::v1_capabilities::ENV_NAME.into(),
                            out.env_value,
                        );
                    } else {
                        let mut m = std::collections::HashMap::new();
                        m.insert(
                            crate::v1_capabilities::ENV_NAME.into(),
                            out.env_value,
                        );
                        spawn_req.env = Some(m);
                    }
                }
                caps_meta = Some(out.metadata);
            }
            Err(e) => {
                sandbox_quota::release_in_workspace(&principal_key, Some(ws_path));
                return CliResponse {
                    status: "400 Bad Request",
                    content_type: "application/json",
                    body: serde_json::json!({ "error": e, "code": "capabilities-invalid" })
                        .to_string(),
                };
            }
        }
    }

    // Spawn through the PROVEN v2 internals (find-or-spawn can't collide —
    // the agent_name is a fresh host-minted `api-…` uuid).
    let result = v2_spawn::spawn_session(spawn_req);
    if result.status != "200 OK" {
        // No session ⇒ no child-exit observer ⇒ release the slot ourselves.
        sandbox_quota::release_in_workspace(&principal_key, Some(ws_path));
        return CliResponse {
            status: result.status,
            content_type: "application/json",
            body: result.body,
        };
    }

    // Parse the spawn response. Can't-happen internal errors after a 200
    // spawn: a live session (and its observer) now exists, so the observer
    // OWNS the quota release — never release here (double-decrement).
    let v: serde_json::Value = match serde_json::from_str(&result.body) {
        Ok(v) => v,
        Err(_) => return CliResponse::internal_error("spawn response was not JSON"),
    };
    let session_id_str = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("");
    let agent_name = v.get("agentName").and_then(|x| x.as_str()).unwrap_or("");
    if session_id_str.is_empty() {
        return CliResponse::internal_error("spawn response carried no sessionId");
    }
    let Some(sid) = SessionId::parse(session_id_str) else {
        return CliResponse::internal_error("spawn response sessionId was malformed");
    };

    // F2: record the owning principal so `GET .../messages` can authorize
    // (default-deny, uniform 404 on mismatch — handle_messages reused as-is).
    crate::sandbox_responses::record_owner(session_id_str, &principal_key);

    // Arm the idle reaper (idempotent; a resume re-arms the clock).
    crate::sandbox_reaper::register(sid, timeout_secs);

    // Slice W3 — SELF-MINTING providers (adapter present, no premint:
    // codex/gemini/pi/cursor/hermes) spawn with NO session identity in argv
    // on a FRESH session, so their `api-…` tab row starts `session_id=NULL`
    // (invisible to list/resume). Schedule the deferred adoption probe to
    // discover + stamp the provider-minted id. Resumes need none (the
    // resume grammar in argv already stamped the row at registration);
    // premint providers need none (the spliced flag stamped it); no-adapter
    // presets GET none — documented-absent (module docs).
    if !is_resume {
        if let Some(adapter) =
            k2_core::workspace::provider_resume::provider_resume_for_command(&spawn_command)
        {
            if adapter.premint_flag().is_none() {
                defer_adopt_api_host_session(
                    adapter.provider,
                    ws_path.to_string(),
                    agent_name.to_string(),
                    principal_key.clone(),
                );
            }
        }
    }

    // Initial prompt transport (prd-host-session-launch-param-prompt-v1):
    // - launch_plan.attached → already on ephemeral exec argv (fire-once);
    //   do NOT start the detached inject thread (A5 — no double-submit).
    // - !attached + non-empty prompt → legacy post-spawn paste inject (D10
    //   fallback for Hermes / unknown / assembly fail).
    // - empty prompt → neither.
    //
    // Injection stack (Phase 0b, paste path only): frozen [`API_SPAWN_PREAMBLE`]
    // + owner `api_guest_policy` + caller prompt. Same logical payload is
    // what launch-param attaches as a single trailing interactive arg.
    // Value never logged.
    let prompt = req.prompt.as_deref().unwrap_or("").trim().to_string();
    if !prompt.is_empty() && !launch_plan.attached {
        let guest = k2_core::workspace::settings::get_api_guest_policy(ws_path);
        let payload = compose_spawn_inject(&guest, &prompt);
        let sid_for_inject = sid;
        let ready_timeout = spawn_prompt_ready_timeout();
        // W4: the spawned agent's readiness dialect via the ONE shared
        // precedence chain (preset-declared `readiness` metadata → static
        // provider table → poll default). Polling providers poll `?2004h`
        // from t=0 and wait 1s after a 0→1 flip; ?2004h LIES for
        // codex/gemini/pi/hermes — polling would inject into their
        // startup dialogs (which EAT text); those profiles wait their
        // settle floor instead, still capped by the ceiling above.
        let inject_profile = policy::resolve_host_injection_profile(ws_path);
        std::thread::spawn(move || {
            let ok = crate::workspace_msg::inject_raw_into_session_with_profile(
                &sid_for_inject,
                &payload,
                ready_timeout,
                &inject_profile,
            );
            log_debug!(
                "[v1-host] post-spawn prompt injection for session={} delivered={}",
                sid_for_inject,
                ok
            );
        });
    } else if launch_plan.attached {
        log_debug!(
            "[v1-host] launch-param attached for session={}; skipping post-spawn inject",
            sid
        );
    }

    // Per-session STREAM token (grid WS) — the caller streams with this,
    // never the API key.
    let stream_tok = stream_token::mint(&sid);
    let grid = format!("/cli/sessions/grid?session={session_id_str}&token={stream_tok}");

    // Spawn / dead-resume response. S4: `resumed` is true only on live path;
    // fresh spawn omits or false; dead re-spawn is false (new PTY).
    let mut body = serde_json::json!({
        "sessionId": session_id_str,
        "agentName": agent_name,
        "workspace": slug,
        "sandbox": "none",
        "stream": { "grid": grid },
    });
    if is_resume {
        body["resumed"] = serde_json::json!(false);
    }
    if let Some(meta) = caps_meta {
        body["capabilities"] = meta;
    }
    body["modelApplied"] = match &model_decision.applied {
        Some(id) => serde_json::json!(id),
        None => serde_json::Value::Null,
    };
    body["modelSource"] = serde_json::to_value(model_decision.source)
        .unwrap_or_else(|_| serde_json::json!("none"));
    CliResponse::ok_json(body.to_string())
}

/// `GET /v1/w/<ws>/host-sessions/queue` — list open spawn-queue jobs for the
/// calling principal in this workspace.
pub(crate) fn handle_v1_host_queue_list(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let jobs = crate::spawn_queue::list_jobs(&ws_path, &principal.display_id());
    let items: Vec<serde_json::Value> = jobs.iter().map(|j| j.to_json()).collect();
    CliResponse::ok_json(
        serde_json::json!({
            "workspace": slug,
            "jobs": items,
            "featureEnabled": crate::spawn_queue::feature_enabled(),
        })
        .to_string(),
    )
}

/// `GET /v1/w/<ws>/host-sessions/queue/<jobId>` — job status (owning principal).
pub(crate) fn handle_v1_host_queue_get(
    principal: &V1Principal,
    ws_raw: &str,
    job_raw: &str,
) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(job_id) = decode_and_validate_segment(job_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let Some(view) = crate::spawn_queue::get_job(&ws_path, &job_id) else {
        return uniform_ws_404();
    };
    // Owning principal only — unknown / unowned same uniform 404.
    if view.principal_id != principal.display_id() {
        return uniform_ws_404();
    }
    CliResponse::ok_json(view.to_json().to_string())
}

/// `POST /v1/w/<ws>/host-sessions/queue/<jobId>/cancel` — cancel a queued job.
/// Empty body OK (same shape as kill). Daemon is GET/POST only — not DELETE.
pub(crate) fn handle_v1_host_queue_cancel(
    principal: &V1Principal,
    ws_raw: &str,
    job_raw: &str,
) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(job_id) = decode_and_validate_segment(job_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match crate::spawn_queue::cancel_job(&ws_path, &job_id, &principal.display_id()) {
        Ok(view) => CliResponse::ok_json(
            serde_json::json!({
                "jobId": view.job_id,
                "status": "cancelled",
                "workspace": slug,
            })
            .to_string(),
        ),
        Err(crate::spawn_queue::CancelError::NotFound) => uniform_ws_404(),
        Err(crate::spawn_queue::CancelError::NotCancellable { status }) => CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: serde_json::json!({
                "error": "job is not cancellable",
                "code": "queue-job-not-cancellable",
                "status": status.as_str(),
            })
            .to_string(),
        },
    }
}

/// `GET /v1/w/<ws>/host-sessions` — list this workspace's api-spawned host
/// sessions (audit). Reads the durable `workspace_tab_sessions` index
/// restricted to the `api-…` namespace; liveness comes from `v2_session_map`.
///
/// COVERAGE (Slice W3, see module docs): premint providers appear
/// immediately; self-minting adapter providers appear once the deferred
/// adoption stamps their provider-minted id (~seconds after boot);
/// no-adapter presets never appear here — their rows stay `session_id=NULL`
/// by honest necessity (nothing K2 can list would be resumable).
pub(crate) fn handle_v1_host_list(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    // Authz FIRST — an unauthorized caller must not even learn the ws exists.
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let rows = match list_api_session_rows(&ws_path) {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(format!("list host sessions: {e}")),
    };

    // F7: one row per sessionId (latest last_seen wins). Historical rows with
    // the same sessionId but older agentName used to all flip live:true when
    // SessionId lookup hit a fresh respawn — integrators must key on
    // sessionId, not sessionId+agentName.
    let mut by_sid: std::collections::BTreeMap<String, (String, i64)> =
        std::collections::BTreeMap::new();
    for (sid, agent, last_seen) in rows {
        match by_sid.get(&sid) {
            Some((_, prev)) if *prev >= last_seen => {}
            _ => {
                by_sid.insert(sid, (agent, last_seen));
            }
        }
    }
    // Drop map entries whose child already exited but never unregistered
    // (restart-orphans / missed ChildExit). Cheap O(live sessions).
    crate::v2_session_map::reconcile_dead_children();

    let sessions: Vec<serde_json::Value> = by_sid
        .into_iter()
        .map(|(sid, (agent, last_seen))| {
            let live = host_session_row_is_live(&sid, &agent);
            serde_json::json!({
                "sessionId": sid,
                "agentName": agent,
                "live": live,
                "lastSeenAt": last_seen,
            })
        })
        .collect();
    CliResponse::ok_json(
        serde_json::json!({ "workspace": slug, "sessions": sessions }).to_string(),
    )
}

/// Durable API-origin rows for a workspace path (`from_api` or `api-…` name).
fn list_api_session_rows(ws_path: &str) -> Result<Vec<(String, String, i64)>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT wts.session_id, wts.agent_name, wts.last_seen_at \
             FROM workspace_tab_sessions wts \
             JOIN projects p ON p.id = wts.project_id \
             WHERE p.path = ?1 \
               AND (wts.from_api = 1 OR wts.agent_name LIKE 'api-%') \
               AND wts.session_id IS NOT NULL \
             ORDER BY wts.last_seen_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let mapped = stmt
        .query_map(rusqlite::params![ws_path], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
        })
        .map_err(|e| e.to_string())?;
    Ok(mapped.flatten().collect())
}

/// Owner-cockpit list: `GET /cli/host-sessions/list?project=<path>`.
/// Same durable index as `GET /v1/w/<ws>/host-sessions` (from_api rows).
pub fn handle_cli_host_sessions_list(params: &std::collections::HashMap<String, String>) -> CliResponse {
    let Some(ws_path) = params
        .get("project")
        .or_else(|| params.get("project_path"))
        .cloned()
        .filter(|s| !s.is_empty())
    else {
        return CliResponse::bad_request("Missing project parameter");
    };
    let rows = match list_api_session_rows(&ws_path) {
        Ok(v) => v,
        Err(e) => return CliResponse::internal_error(format!("list host sessions: {e}")),
    };
    let mut by_sid: std::collections::BTreeMap<String, (String, i64)> =
        std::collections::BTreeMap::new();
    for (sid, agent, last_seen) in rows {
        match by_sid.get(&sid) {
            Some((_, prev)) if *prev >= last_seen => {}
            _ => {
                by_sid.insert(sid, (agent, last_seen));
            }
        }
    }
    crate::v2_session_map::reconcile_dead_children();
    let sessions: Vec<serde_json::Value> = by_sid
        .into_iter()
        .map(|(sid, (agent, last_seen))| {
            let live = host_session_row_is_live(&sid, &agent);
            serde_json::json!({
                "sessionId": sid,
                "agentName": agent,
                "live": live,
                "lastSeenAt": last_seen,
                "fromApi": true,
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::json!(sessions).to_string())
}

/// Whether a host-sessions list row is truly live (backing PTY child alive).
///
/// Map presence alone is insufficient: a KillMode restart (or any death
/// without ChildExit→unregister) left phantom `live:true` rows (scout
/// 0.40.78 finding — S9/list lag). Always require [`DaemonPtySession::is_child_alive`].
///
/// Identity:
/// - UUID `sessionId` matching the map entry's daemon SessionId (premint / resume)
/// - else live under `agentName` for adopted provider-minted ids (DB sid ≠ daemon sid)
fn host_session_row_is_live(sid: &str, agent: &str) -> bool {
    if let Some(id) = SessionId::parse(sid) {
        if let Some(s) = crate::v2_session_map::lookup_by_session_id(&id) {
            return s.is_child_alive();
        }
    }
    let Some(s) = crate::v2_session_map::lookup_by_agent_name(agent) else {
        return false;
    };
    if !s.is_child_alive() {
        return false;
    }
    // Premint / UUID resume: agent live only counts if this row's sid is the PTY's id.
    if let Some(id) = SessionId::parse(sid) {
        if s.session_id == id {
            return true;
        }
        // Adopted self-minting providers: stamped sid is provider-minted; PTY
        // uses a different daemon SessionId. Host `api-…` names are 1:1 with
        // one spawn, so agent-name liveness is the row's liveness.
        return agent.starts_with("api-");
    }
    // Non-UUID sid (e.g. hermes) — agent_name is the only handle.
    true
}

/// `POST /v1/w/<ws>/host-sessions/<id>` — MESSAGE-LIVE: inject the caller's
/// prompt into a LIVE host session's PTY. A dead/unknown/unowned/canonical id
/// is a uniform 404 (resume rides the spawn route's `{"session": id}`).
pub(crate) fn handle_v1_host_message(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
    body: &[u8],
) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(sid_seg) = decode_and_validate_segment(sid_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // CANONICAL OFF-LIMITS — never drive a workspace's canonical session here.
    if session_is_canonical(&sid_seg) {
        return uniform_ws_404();
    }
    // OWNERSHIP (default-deny): only the principal that spawned this session
    // may message it — unknown and unowned are the SAME uniform 404.
    let requester = principal.display_id();
    match crate::sandbox_responses::owner_of(&sid_seg) {
        Some(owner) if owner == requester => {}
        _ => return uniform_ws_404(),
    }
    // LIVENESS + WORKSPACE PIN: the session must be live AND rooted in THIS
    // workspace (a principal granted two workspaces can't cross-address).
    // Slice W3: resolves BOTH id shapes — the daemon SessionId (premint
    // providers) and an ADOPTED provider-minted id (self-minting providers,
    // whose owner record was written at adoption time so the gate above
    // already vouched for it).
    let Some(live) = lookup_live_host_session(&ws_path, &sid_seg) else {
        return uniform_ws_404();
    };
    let cwd_matches = live
        .cwd
        .as_ref()
        .map(|p| p.to_string_lossy() == ws_path)
        .unwrap_or(false);
    if !cwd_matches {
        return uniform_ws_404();
    }

    // Body: prompt + optional capabilities[] (re-mint file) + optional timeout_secs
    // for JWT lifetime. Guest policy is host-resolved (never from the caller).
    let v = serde_json::from_slice::<serde_json::Value>(body).unwrap_or(serde_json::json!({}));
    let prompt = v
        .get("prompt")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let timeout_secs = v
        .get("timeout_secs")
        .and_then(|x| x.as_u64());
    let caps = v.get("capabilities").cloned();
    let model = v.get("model").and_then(|x| x.as_str());
    deliver_into_live(
        &live.session_id,
        &sid_seg,
        prompt.trim(),
        &ws_path,
        &slug,
        &requester,
        caps.as_ref(),
        timeout_secs,
        model,
    )
}

/// `GET /v1/w/<ws>/host-sessions/<id>/messages?since=<seq>` — drain the
/// in-session agent's `k2 respond` log. Authorizes the workspace, refuses the
/// canonical session, then reuses the F2 drain
/// ([`crate::v1_sandboxes::handle_messages`]) whose default-deny owner check
/// still applies — a workspace-authorized caller reads only sessions it OWNS.
pub(crate) fn handle_v1_host_messages(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
    since: u64,
) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(sid) = decode_and_validate_segment(sid_raw) else {
        return uniform_ws_404();
    };
    if let Err(resp) = resolve_authorized_workspace(principal, &slug) {
        return resp;
    }
    if session_is_canonical(&sid) {
        return uniform_ws_404();
    }
    crate::v1_sandboxes::handle_messages(principal, &sid, since)
}

/// Kill-path ownership tombstones (sessionId → principal.display_id).
///
/// ChildExit runs `sandbox_responses::evict` after every PTY death, which
/// drops the live owner record. Without a tombstone, a second kill on an
/// already-killed session becomes a uniform 404 and breaks K6
/// idempotency (`killed:false, reason:"not_live"`). Tombstones are
/// principal-keyed so other callers still 404 (no existence oracle). Cap
/// the map so a long-lived daemon cannot grow unbounded.
fn kill_tombstones() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
        std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

const KILL_TOMBSTONE_CAP: usize = 8192;

fn record_kill_tombstone(session_id: &str, principal: &str) {
    let Ok(mut m) = kill_tombstones().lock() else {
        return;
    };
    if m.len() >= KILL_TOMBSTONE_CAP {
        // Bound growth: drop an arbitrary half when full (daemon-local
        // cache; restart clears). Prefer a cheap bound over LRU complexity.
        let drop_n = m.len() / 2;
        let keys: Vec<String> = m.keys().take(drop_n).cloned().collect();
        for k in keys {
            m.remove(&k);
        }
    }
    m.insert(session_id.to_string(), principal.to_string());
}

fn kill_tombstone_owner(session_id: &str) -> Option<String> {
    kill_tombstones()
        .lock()
        .ok()
        .and_then(|m| m.get(session_id).cloned())
}

/// True when `requester` owns `session_id` via the live owner map OR a
/// prior kill tombstone (K6 second-kill path after ChildExit eviction).
///
/// **Ephemeral only** — wiped on daemon restart. Kill path uses
/// [`principal_may_kill_host_session`] (durable fallback + Owner floor).
fn principal_owns_host_session(session_id: &str, requester: &str) -> bool {
    match crate::sandbox_responses::owner_of(session_id) {
        Some(owner) if owner == requester => true,
        _ => matches!(kill_tombstone_owner(session_id), Some(owner) if owner == requester),
    }
}

/// Durable `agent_name` for an `api-*` tab row of this session in this
/// workspace. Survives daemon restart (unlike the in-memory owner map).
fn durable_api_agent_name(ws_path: &str, session_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT wts.agent_name FROM workspace_tab_sessions wts \
             JOIN projects p ON p.id = wts.project_id \
             WHERE p.path = ?1 AND wts.session_id = ?2 \
               AND wts.agent_name LIKE 'api-%' \
             ORDER BY wts.last_seen_at DESC \
             LIMIT 1",
        )
        .ok()?;
    stmt.query_row(rusqlite::params![ws_path, session_id], |r| r.get::<_, String>(0))
        .ok()
}

/// Drop durable `api-%` tab rows for this session in this workspace.
/// Returns rows deleted (orphan list cleanup after not_live kill).
fn clear_durable_api_host_rows(ws_path: &str, session_id: &str) -> usize {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "DELETE FROM workspace_tab_sessions \
         WHERE project_id = (SELECT id FROM projects WHERE path = ?1 LIMIT 1) \
           AND session_id = ?2 \
           AND agent_name LIKE 'api-%'",
        rusqlite::params![ws_path, session_id],
    )
    .unwrap_or(0)
}

/// Infer principal key from host-minted `api-{principal}-{uuid}` agent_name.
fn principal_key_from_api_agent_name(agent_name: &str) -> Option<String> {
    let rest = agent_name.strip_prefix("api-")?;
    // Mint: format!("api-{}-{}", principal, Uuid) — UUID is 36 chars.
    if rest.len() > 37 && rest.as_bytes().get(rest.len() - 37) == Some(&b'-') {
        return Some(rest[..rest.len() - 37].to_string());
    }
    rest.split_once('-').map(|(p, _)| p.to_string())
}

/// Best-effort quota release for a not_live orphan when ChildExit never ran.
fn best_effort_quota_release_from_agent(ws_path: &str, agent_name: &str) {
    if let Some(pk) = principal_key_from_api_agent_name(agent_name) {
        crate::sandbox_quota::release_in_workspace(&pk, Some(ws_path));
    }
}

/// Kill / status auth contract (Scout orphan cleanup / 0.40.87 + status PRD):
///
/// After workspace grant is proven:
/// 1. **Exact spawning principal** — live owner map, kill tombstone, OR durable
///    agent_name prefix `api-{display_id}-` (survives restart).
/// 2. **Owner floor** — `V1Principal::Owner` may kill/observe **any** durable
///    `api-%` host-session in that authorized workspace (reconciler / spend-cap).
///
/// API keys cannot observe another key's sessions. No durable row + no live
/// owner/tombstone → uniform 404 (not an existence oracle for random ids).
fn principal_may_kill_host_session(
    principal: &V1Principal,
    ws_path: &str,
    session_id: &str,
) -> bool {
    principal_may_observe_host_session(principal, ws_path, session_id)
}

/// Read-only twin of [`principal_may_kill_host_session`] — same rules (exact
/// spawner via map/tombstone/durable prefix, or Owner floor on durable
/// `api-%`). Status uses this name so call sites read as observe, not kill.
fn principal_may_observe_host_session(
    principal: &V1Principal,
    ws_path: &str,
    session_id: &str,
) -> bool {
    let requester = principal.display_id();
    if principal_owns_host_session(session_id, &requester) {
        return true;
    }
    let Some(agent) = durable_api_agent_name(ws_path, session_id) else {
        return false;
    };
    // Host-minted `api-{principal}-{uuid}` (principal may contain hyphens).
    let prefix = format!("api-{}-", requester);
    if agent.starts_with(&prefix) {
        return true;
    }
    // Daemon owner may observe any durable api-* host session in an authorized
    // workspace (Scout reconciler needs this after restart / cross-key sweep).
    matches!(principal, V1Principal::Owner)
}

/// `POST /v1/w/<ws>/host-sessions/<id>/kill` — force-stop a live host-session
/// PTY for integrator spend-cap / deliberate teardown.
///
/// Authz: workspace grant + [`principal_may_kill_host_session`] (exact spawner
/// via live map / tombstone / durable `api-{principal}-*`, **or** daemon Owner
/// floor for any durable `api-%` in that workspace). Canonical / unknown /
/// unowned / wrong-ws → uniform 404. Empty body OK.
///
/// Idempotent for an authorized session that is no longer live → 200
/// `{"sessionId","killed":false,"reason":"not_live","indexCleared":bool}` plus
/// best-effort durable index / map cleanup (Scout restart orphans). Live
/// teardown uses
/// [`crate::sandbox_reaper::force_teardown_host_session_preserve_index`]
/// (keeps durable row for dead-resume). Grace expiry also **retains** the
/// durable index (same re-wake contract — only explicit not_live `/kill`
/// clears). Cap file / JWTs left in place (K8). Kill tombstone is stamped
/// here only (auto Grace must not tombstone).
pub(crate) fn handle_v1_host_kill(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(sid_seg) = decode_and_validate_segment(sid_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // CANONICAL OFF-LIMITS — never drive a workspace's canonical session here.
    if session_is_canonical(&sid_seg) {
        return uniform_ws_404();
    }
    // Kill auth: exact spawner (map/tombstone/durable prefix) OR Owner floor
    // on durable api-* rows. Unknown/unowned → same uniform 404 as bad ws.
    let requester = principal.display_id();
    if !principal_may_kill_host_session(principal, &ws_path, &sid_seg) {
        return uniform_ws_404();
    }
    // Authorized but not live → K6 idempotent success + clear durable orphans
    // so list does not retain ghosts after restart (Scout 68-orphan case).
    let Some(live) = lookup_live_host_session(&ws_path, &sid_seg) else {
        let agent = durable_api_agent_name(&ws_path, &sid_seg);
        let had_durable = agent.is_some();
        // Prefer shared chokepoint when we have a parseable daemon SessionId
        // (also clears durable api-* tab rows inside force_teardown).
        if let Some(parsed) = SessionId::parse(&sid_seg) {
            crate::sandbox_reaper::force_teardown_host_session_ctx(
                &parsed,
                &ws_path,
                &sid_seg,
            );
        } else if let Some(ref name) = agent {
            let _ = crate::v2_session_map::unregister(name);
        }
        let cleared = clear_durable_api_host_rows(&ws_path, &sid_seg);
        // Quota: ChildExit may never have run after a restart. Release once
        // for not_live only — live path still leaves release to child-exit.
        if let Some(ref name) = agent {
            best_effort_quota_release_from_agent(&ws_path, name);
        }
        crate::sandbox_responses::evict(&sid_seg);
        record_kill_tombstone(&sid_seg, &requester);
        return CliResponse::ok_json(
            serde_json::json!({
                "sessionId": sid_seg,
                "killed": false,
                "reason": "not_live",
                "indexCleared": had_durable || cleared > 0,
            })
            .to_string(),
        );
    };
    // Workspace pin: a principal granted two workspaces can't cross-kill.
    let cwd_matches = live
        .cwd
        .as_ref()
        .map(|p| p.to_string_lossy() == ws_path)
        .unwrap_or(false);
    if !cwd_matches {
        return uniform_ws_404();
    }

    // Live kill: stop the PTY / map / reaper keys, but **keep** the durable
    // api-% tab row so `POST …/host-sessions {"session": id}` can dead-resume
    // under the same conversation handle (kill PRD §6 acceptance + launch-param
    // resume arm). Grace expiry still clears the index (work completed).
    // Quota is still released by the child-exit observer — force_teardown
    // must not double-release.
    crate::sandbox_reaper::force_teardown_host_session_preserve_index(
        &live.session_id,
        &ws_path,
        &sid_seg,
    );
    // Stamp tombstone BEFORE returning so a racing second kill sees ownership
    // even if ChildExit already ran `sandbox_responses::evict`.
    record_kill_tombstone(&sid_seg, &requester);

    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": sid_seg,
            "killed": true,
            // Index retained for dead-resume (not a list ghost — live:false
            // is honest until resume re-spawns).
            "resumable": true,
        })
        .to_string(),
    )
}

/// Durable `command` for an `api-%` tab row of this session (harness hint for
/// provider transcript probes). `None` when no row / no command stored.
fn durable_api_command(ws_path: &str, session_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let mut stmt = conn
        .prepare(
            "SELECT wts.command FROM workspace_tab_sessions wts \
             JOIN projects p ON p.id = wts.project_id \
             WHERE p.path = ?1 AND wts.session_id = ?2 \
               AND wts.agent_name LIKE 'api-%' \
             ORDER BY wts.last_seen_at DESC \
             LIMIT 1",
        )
        .ok()?;
    stmt.query_row(rusqlite::params![ws_path, session_id], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
    .filter(|c| !c.trim().is_empty())
}

/// Host-side evidence that a provider wrote a session/transcript file for
/// `session_id` under `ws_path`. Prefer the durable row's command when known;
/// otherwise probe common providers (claude, grok, cursor) until one hits.
fn provider_session_file_exists(
    ws_path: &str,
    session_id: &str,
    command_hint: Option<&str>,
) -> bool {
    use k2_core::workspace::provider_resume::{
        provider_resume_for_command, provider_resume_for_provider,
    };
    if let Some(cmd) = command_hint {
        if let Some(adapter) = provider_resume_for_command(cmd) {
            if adapter.session_file_exists(session_id, ws_path) {
                return true;
            }
        }
        // Stored harness may be the provider key rather than the binary.
        if let Some(adapter) = provider_resume_for_provider(cmd) {
            if adapter.session_file_exists(session_id, ws_path) {
                return true;
            }
        }
    }
    for p in ["claude", "grok", "cursor"] {
        if let Some(adapter) = provider_resume_for_provider(p) {
            if adapter.session_file_exists(session_id, ws_path) {
                return true;
            }
        }
    }
    false
}

/// Reaper phase string for status: `"none"` | `"working"` | `"grace"`.
/// Tries the caller-facing session id, then the live PTY's daemon SessionId
/// (adopted self-mint ids may reaper-register under the daemon id).
fn reaper_phase_for(ws_path: &str, session_id: &str) -> &'static str {
    if let Some(parsed) = SessionId::parse(session_id) {
        if let Some(label) = crate::sandbox_reaper::phase_label(&parsed) {
            return label;
        }
    }
    if let Some(live) = lookup_live_host_session(ws_path, session_id) {
        if let Some(label) = crate::sandbox_reaper::phase_label(&live.session_id) {
            return label;
        }
    }
    "none"
}

/// Pure phase resolver for host-session status (priority order from PRD).
///
/// Returns `None` only when the session is not "known" to the principal in
/// any form the handler already authorized — call sites should 404.
///
/// Priority:
/// 1. `live` → `working`
/// 2. reaper grace → `grace`
/// 3. `started && !live` → `finished`
/// 4. known (durable / live-owner map / Owner durable) && !started → `never_started`
/// 5. kill tombstone only → `gone`
fn compute_host_status_phase(
    live: bool,
    reaper_grace: bool,
    started: bool,
    known_durable_or_owned: bool,
    tombstone_only: bool,
) -> Option<&'static str> {
    if live {
        return Some("working");
    }
    if reaper_grace {
        return Some("grace");
    }
    if started {
        return Some("finished");
    }
    if known_durable_or_owned {
        return Some("never_started");
    }
    if tombstone_only {
        return Some("gone");
    }
    None
}

/// `GET /v1/w/<ws>/host-sessions/<id>` — read-only status for Scout reconciler:
/// `started` / `live` / `phase` / `latest_seq` / `reaper` / `durable`.
///
/// Authz mirrors kill ([`principal_may_observe_host_session`]): exact spawner
/// (live owner map, kill tombstone, durable `api-{principal}-*`) or Owner floor
/// on durable `api-%`. Canonical / unowned / unknown → uniform 404.
///
/// **No side effects** — does not kill, stamp reaper, inject, remint, or
/// write kill tombstones / durable rows.
///
/// Field naming (caps-recovery addendum §3): **`latest_seq`** snake_case only
/// (matches drain `GET …/messages`); do not dual-emit `latestSeq`.
pub(crate) fn handle_v1_host_status(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
) -> CliResponse {
    if let Err(resp) = require_host_sessions(principal) {
        return resp;
    }
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    let Some(sid_seg) = decode_and_validate_segment(sid_raw) else {
        return uniform_ws_404();
    };
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    if session_is_canonical(&sid_seg) {
        return uniform_ws_404();
    }
    if !principal_may_observe_host_session(principal, &ws_path, &sid_seg) {
        return uniform_ws_404();
    }

    let requester = principal.display_id();
    let agent = durable_api_agent_name(&ws_path, &sid_seg);
    let durable = agent.is_some();
    let command_hint = durable_api_command(&ws_path, &sid_seg);
    let live_owner = matches!(
        crate::sandbox_responses::owner_of(&sid_seg),
        Some(ref o) if o == &requester
    );
    let has_tombstone = matches!(
        kill_tombstone_owner(&sid_seg),
        Some(ref o) if o == &requester
    );

    // Liveness: prefer agent-name-aware check when durable row exists; else
    // try live map by session id / adopted agent lookup.
    let live = if let Some(ref a) = agent {
        host_session_row_is_live(&sid_seg, a)
    } else {
        lookup_live_host_session(&ws_path, &sid_seg)
            .map(|s| s.is_child_alive())
            .unwrap_or(false)
    };

    let latest_seq = crate::sandbox_responses::latest_seq(&sid_seg);
    let file_exists =
        provider_session_file_exists(&ws_path, &sid_seg, command_hint.as_deref());
    // PRD §5 / addendum §3.2: live OR latest_seq > 0 OR provider session file.
    let started = live || latest_seq > 0 || file_exists;

    let reaper = reaper_phase_for(&ws_path, &sid_seg);
    let reaper_grace = reaper == "grace";

    // "known" for never_started: durable row, live owner map, or Owner floor
    // already required durable for observe when not exact-owner.
    let known_durable_or_owned = durable || live_owner;
    // Tombstone-only: authorized via kill tombstone, no durable, no live owner.
    let tombstone_only = has_tombstone && !durable && !live_owner;

    let Some(phase) = compute_host_status_phase(
        live,
        reaper_grace,
        started,
        known_durable_or_owned,
        tombstone_only,
    ) else {
        // Authorized via Owner+durable or prefix should always be known; if
        // we somehow pass observe without a phase, fail closed (no oracle).
        return uniform_ws_404();
    };

    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": sid_seg,
            "workspace": slug,
            "agentName": agent,
            "live": live,
            "started": started,
            "phase": phase,
            "latest_seq": latest_seq,
            "reaper": reaper,
            "durable": durable,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shared in-memory test DB (process-global) — unique `v1host-` prefixes
    // keep these rows disjoint from every other module's tests.

    fn apik(id: &str, grant: Option<&str>) -> V1Principal {
        apik_caps(id, grant, k2_core::api_keys::ApiCapabilities::all())
    }

    fn apik_caps(
        id: &str,
        grant: Option<&str>,
        capabilities: k2_core::api_keys::ApiCapabilities,
    ) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: grant.map(str::to_string),
            capabilities,
        })
    }

    fn insert_project(name: &str, path: &str) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, name, path],
        )
        .expect("insert project");
        id
    }

    fn insert_canonical_session(project_id: &str, session_id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO workspace_sessions \
                 (id, project_id, terminal_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', 'system', 'active', unixepoch())",
            rusqlite::params![uuid::Uuid::new_v4().to_string(), project_id, "term-1", session_id],
        )
        .expect("insert canonical session");
    }

    fn insert_host_tab_session(project_id: &str, session_id: &str, agent_name: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO workspace_tab_sessions \
                 (project_id, pane_group_id, agent_name, session_id, command) \
             VALUES (?1, ?2, ?3, ?4, 'claude')",
            rusqlite::params![project_id, agent_name, agent_name, session_id],
        )
        .expect("insert tab session row");
    }

    /// The API contract preamble is FROZEN — external callers and the
    /// integration suite pin these exact bytes. Change it only with a
    /// deliberate, coordinated contract bump.
    #[test]
    fn api_spawn_preamble_is_byte_frozen() {
        assert_eq!(
            API_SPAWN_PREAMBLE,
            "[K2 API] You were invoked through the K2 API. Report progress and results \
             back to the caller by running: k2 respond '<your message>' — and send your \
             final answer with: k2 respond --final '<your answer>'. The caller cannot \
             see your terminal; only k2 respond output reaches them.",
        );
    }

    /// Phase 0b inject ORDERING: spawn = preamble → guest policy → caller;
    /// follow-up = guest policy → caller (no preamble). Pure compose helpers.
    #[test]
    fn inject_compose_orders_preamble_guest_then_caller() {
        let guest = "GUEST-POLICY-MARKER";
        let caller = "CALLER-PROMPT-MARKER";
        let spawn = compose_spawn_inject(guest, caller);
        assert_eq!(
            spawn,
            format!("{API_SPAWN_PREAMBLE}\n\n{guest}\n\n{caller}"),
            "spawn stack must be preamble, blank, guest, blank, caller"
        );
        assert!(
            spawn.find(API_SPAWN_PREAMBLE).unwrap() < spawn.find(guest).unwrap(),
            "preamble before guest"
        );
        assert!(
            spawn.find(guest).unwrap() < spawn.find(caller).unwrap(),
            "guest before caller"
        );

        let follow = compose_followup_inject(guest, caller);
        assert_eq!(
            follow,
            format!("{guest}\n\n{caller}"),
            "follow-up is guest then caller only"
        );
        assert!(
            !follow.contains(API_SPAWN_PREAMBLE),
            "follow-up must NOT re-send the frozen spawn preamble"
        );
        assert!(
            follow.find(guest).unwrap() < follow.find(caller).unwrap(),
            "guest before caller on follow-up"
        );
    }

    /// Phase 0b: request body cannot carry/override guest policy —
    /// `ApiHostSessionRequest` has no such field; unknown keys are ignored
    /// by serde, and the host always reads policy from workspace settings.
    #[test]
    fn request_body_cannot_set_or_override_guest_policy() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-guest-override";
        insert_project("v1host-guest-override", ws);

        // Owner-configured policy in settings.
        k2_core::workspace::settings::update_project_setting(
            ws,
            "api_guest_policy",
            "OWNER-CONFIGURED-GUEST-POLICY",
        )
        .expect("set owner guest policy");
        assert_eq!(
            k2_core::workspace::settings::get_api_guest_policy(ws),
            "OWNER-CONFIGURED-GUEST-POLICY"
        );

        // Body that *tries* to smuggle a different policy + a prompt.
        let body = br#"{
            "prompt": "hi",
            "api_guest_policy": "ATTACKER-POLICY",
            "apiGuestPolicy": "ATTACKER-POLICY",
            "guest_policy": "ATTACKER-POLICY"
        }"#;
        let req = match parse_body(body) {
            Ok(r) => r,
            Err(e) => panic!("body must parse (unknown fields ignored); status={}", e.status),
        };
        assert_eq!(req.prompt.as_deref(), Some("hi"));
        // No guest-policy field exists on the request type — only these hints.
        let _ = (
            &req.prompt,
            &req.cols,
            &req.rows,
            &req.timeout_secs,
            &req.session,
        );

        // Julie/Scout: timeoutSecs camelCase must not silently drop to default.
        let camel = br#"{"timeoutSecs":960,"prompt":"x"}"#;
        let req_camel = match parse_body(camel) {
            Ok(r) => r,
            Err(e) => panic!("camel timeoutSecs must parse; status={}", e.status),
        };
        assert_eq!(req_camel.timeout_secs, Some(960));
        let snake = br#"{"timeout_secs":960}"#;
        let req_snake = match parse_body(snake) {
            Ok(r) => r,
            Err(e) => panic!("snake timeout_secs must parse; status={}", e.status),
        };
        assert_eq!(req_snake.timeout_secs, Some(960));
        // JWT lifetime for spawn is min(normalize(timeout), 3600) — 960 stays 960.
        let exp = crate::sandbox_reaper::normalize_timeout(req_camel.timeout_secs)
            .min(3600)
            .max(1);
        assert_eq!(exp, 960, "cap JWT lifetime must track timeout_secs=960");

        // Host-resolved policy is still the owner's, never the body.
        let resolved = k2_core::workspace::settings::get_api_guest_policy(ws);
        assert_eq!(resolved, "OWNER-CONFIGURED-GUEST-POLICY");
        assert!(!resolved.contains("ATTACKER"));

        let spawn = compose_spawn_inject(&resolved, req.prompt.as_deref().unwrap_or(""));
        assert!(spawn.contains("OWNER-CONFIGURED-GUEST-POLICY"));
        assert!(!spawn.contains("ATTACKER-POLICY"));
        assert!(spawn.contains(API_SPAWN_PREAMBLE));

        let follow = compose_followup_inject(&resolved, "follow-up");
        assert!(follow.contains("OWNER-CONFIGURED-GUEST-POLICY"));
        assert!(!follow.contains("ATTACKER-POLICY"));
        assert!(!follow.contains(API_SPAWN_PREAMBLE));
    }

    /// Empty/NULL workspace setting falls through to the platform default.
    #[test]
    fn guest_policy_defaults_when_unset() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-guest-default";
        insert_project("v1host-guest-default", ws);
        assert_eq!(
            k2_core::workspace::settings::get_api_guest_policy(ws),
            k2_core::workspace::settings::DEFAULT_API_GUEST_POLICY,
        );
        assert_eq!(
            k2_core::workspace::settings::get_api_guest_policy_raw(ws),
            None,
        );
        // Empty string also → default.
        k2_core::workspace::settings::update_project_setting(ws, "api_guest_policy", "")
            .expect("clear");
        assert_eq!(
            k2_core::workspace::settings::get_api_guest_policy(ws),
            k2_core::workspace::settings::DEFAULT_API_GUEST_POLICY,
        );
    }

    /// Every door-block case returns the UNIFORM 404 ("no such workspace"):
    /// malformed slug, unknown slug, ungranted key — before any spawn work.
    #[test]
    fn new_blocks_uniformly_at_the_door() {
        k2_core::db::init_for_tests();
        insert_project("v1host-new-door", "/tmp/k2-v1host-new-door");

        for (principal, ws) in [
            (V1Principal::Owner, "%2e%2e"),               // malformed
            (V1Principal::Owner, "v1host-ghost-none"),    // unknown
        ] {
            let r = handle_v1_host_new(&principal, ws, b"{}");
            assert_eq!(r.status, "404 Not Found", "ws={ws}");
            assert!(r.body.contains("no such workspace"), "uniform body: {}", r.body);
        }
        // Ungranted key on an EXISTING ws → identical 404 (no oracle).
        let ungranted = apik("k-hs-u", Some(r#"["v1host-other"]"#));
        let r = handle_v1_host_new(&ungranted, "v1host-new-door", b"{}");
        assert_eq!(r.status, "404 Not Found");
        assert!(r.body.contains("no such workspace"));
    }

    /// Phase 0 — a key missing `host_sessions` gets the same uniform 404 as
    /// an ungranted key (no existence oracle), even with a full grant.
    #[test]
    fn host_sessions_require_capability() {
        k2_core::db::init_for_tests();
        insert_project("v1host-cap-deny", "/tmp/k2-v1host-cap-deny");
        let no_host = apik_caps(
            "k-hs-no-cap",
            Some("*"),
            k2_core::api_keys::ApiCapabilities {
                host_sessions: false,
                canonical_message: true,
                sandboxes: true,
            },
        );
        for call in [
            handle_v1_host_new(&no_host, "v1host-cap-deny", b"{}"),
            handle_v1_host_list(&no_host, "v1host-cap-deny"),
            handle_v1_host_message(&no_host, "v1host-cap-deny", "any-sid", b"{}"),
            handle_v1_host_status(&no_host, "v1host-cap-deny", "any-sid"),
            handle_v1_host_messages(&no_host, "v1host-cap-deny", "any-sid", 0),
            handle_v1_host_kill(&no_host, "v1host-cap-deny", "any-sid"),
        ] {
            assert_eq!(call.status, "404 Not Found", "body={}", call.body);
            assert!(
                call.body.contains("no such workspace"),
                "uniform body: {}",
                call.body
            );
        }
        // Owner always has the capability (reaches past the cap gate — may
        // still 404 for other reasons, but not with a missing-cap path alone
        // when the workspace is known: list is a 200 empty).
        let list = handle_v1_host_list(&V1Principal::Owner, "v1host-cap-deny");
        assert_eq!(list.status, "200 OK", "owner list body={}", list.body);
    }

    /// Malformed JSON body → 400 (never a spawn).
    #[test]
    fn new_rejects_malformed_json() {
        k2_core::db::init_for_tests();
        insert_project("v1host-badjson", "/tmp/k2-v1host-badjson");
        let r = handle_v1_host_new(&V1Principal::Owner, "v1host-badjson", b"{not json");
        assert_eq!(r.status, "400 Bad Request", "body={}", r.body);
    }

    /// Resume guards: a canonical id, a non-UUID id, and an id this family
    /// never spawned in this workspace are ALL the uniform 404.
    #[test]
    fn resume_guards_canonical_foreign_and_malformed_ids() {
        k2_core::db::init_for_tests();
        let pid = insert_project("v1host-resume-guard", "/tmp/k2-v1host-resume-guard");
        insert_canonical_session(&pid, "11111111-2222-3333-4444-555555555555");

        // Canonical id → off-limits.
        let body = br#"{"session":"11111111-2222-3333-4444-555555555555"}"#;
        assert_eq!(
            handle_v1_host_new(&V1Principal::Owner, "v1host-resume-guard", body).status,
            "404 Not Found",
        );
        // Unknown (never api-spawned here) id → 404.
        let body = br#"{"session":"99999999-8888-7777-6666-555555555555"}"#;
        assert_eq!(
            handle_v1_host_new(&V1Principal::Owner, "v1host-resume-guard", body).status,
            "404 Not Found",
        );
        // Malformed segment → 404.
        let body = br#"{"session":"a/b"}"#;
        assert_eq!(
            handle_v1_host_new(&V1Principal::Owner, "v1host-resume-guard", body).status,
            "404 Not Found",
        );
        // A HUMAN tab's session id (agent_name NOT api-…) is NOT resumable.
        insert_host_tab_session(&pid, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "tab-human-1");
        let body = br#"{"session":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}"#;
        assert_eq!(
            handle_v1_host_new(&V1Principal::Owner, "v1host-resume-guard", body).status,
            "404 Not Found",
            "human tab transcripts are not resumable via the host-sessions API",
        );
    }

    /// `host_session_resumable` admits exactly this-workspace `api-…` rows.
    #[test]
    fn host_session_resumable_scopes_by_workspace_and_namespace() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-resumable";
        let pid = insert_project("v1host-resumable", ws);
        let other_pid = insert_project("v1host-resumable-other", "/tmp/k2-v1host-resumable-other");

        let sid = "12121212-3434-5656-7878-909090909090";
        insert_host_tab_session(&pid, sid, "api-owner-abc");
        assert!(host_session_resumable(ws, sid), "own api session resumes");
        assert!(
            !host_session_resumable("/tmp/k2-v1host-resumable-other", sid),
            "same id addressed via ANOTHER workspace must not resolve"
        );
        // An api session of the OTHER workspace is invisible here.
        let foreign = "21212121-4343-6565-8787-090909090909";
        insert_host_tab_session(&other_pid, foreign, "api-owner-def");
        assert!(!host_session_resumable(ws, foreign));
    }

    /// message-live: canonical → 404; unknown/unowned → 404; wrong-owner → 404.
    #[test]
    fn message_live_refuses_canonical_unknown_and_unowned() {
        k2_core::db::init_for_tests();
        let pid = insert_project("v1host-msg", "/tmp/k2-v1host-msg");
        insert_canonical_session(&pid, "v1host-msg-canon");

        assert_eq!(
            handle_v1_host_message(&V1Principal::Owner, "v1host-msg", "v1host-msg-canon", b"{}")
                .status,
            "404 Not Found",
        );
        // Unknown session (no owner record) → 404.
        assert_eq!(
            handle_v1_host_message(&V1Principal::Owner, "v1host-msg", "v1host-msg-nope", b"{}")
                .status,
            "404 Not Found",
        );
        // Owned by a DIFFERENT principal → identical 404 (no oracle).
        crate::sandbox_responses::record_owner("v1host-msg-owned", "key-somebody-else");
        assert_eq!(
            handle_v1_host_message(&V1Principal::Owner, "v1host-msg", "v1host-msg-owned", b"{}")
                .status,
            "404 Not Found",
        );
        // Owned by THIS principal but NOT LIVE → still 404 (message-live only).
        crate::sandbox_responses::record_owner("f0f0f0f0-0f0f-0f0f-0f0f-f0f0f0f0f0f0", "owner");
        assert_eq!(
            handle_v1_host_message(
                &V1Principal::Owner,
                "v1host-msg",
                "f0f0f0f0-0f0f-0f0f-0f0f-f0f0f0f0f0f0",
                b"{}"
            )
            .status,
            "404 Not Found",
        );
    }

    /// kill: canonical / unknown / unowned → uniform 404; owned-but-not-live
    /// is the idempotent 200 `{killed:false, reason:"not_live"}` (K6).
    #[test]
    fn kill_refuses_canonical_unknown_unowned_and_idempts_dead() {
        k2_core::db::init_for_tests();
        let pid = insert_project("v1host-kill", "/tmp/k2-v1host-kill");
        insert_canonical_session(&pid, "v1host-kill-canon");

        assert_eq!(
            handle_v1_host_kill(&V1Principal::Owner, "v1host-kill", "v1host-kill-canon").status,
            "404 Not Found",
        );
        // Unknown session (no owner record, no durable row) → 404.
        assert_eq!(
            handle_v1_host_kill(&V1Principal::Owner, "v1host-kill", "v1host-kill-nope").status,
            "404 Not Found",
        );
        // Live-map owned by a DIFFERENT principal, no durable row → 404 for
        // Owner floor too (no existence oracle for map-only ghosts).
        crate::sandbox_responses::record_owner("v1host-kill-owned", "key-somebody-else");
        assert_eq!(
            handle_v1_host_kill(&V1Principal::Owner, "v1host-kill", "v1host-kill-owned").status,
            "404 Not Found",
        );
        // Owned by THIS principal but NOT LIVE → 200 killed:false not_live.
        crate::sandbox_responses::record_owner("v1host-kill-dead", "owner");
        let r = handle_v1_host_kill(&V1Principal::Owner, "v1host-kill", "v1host-kill-dead");
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["sessionId"], "v1host-kill-dead");
        assert_eq!(v["killed"], false);
        assert_eq!(v["reason"], "not_live");
        // Ungranted key never reaches kill (uniform 404).
        let ungranted = apik("k-hk-none", None);
        crate::sandbox_responses::record_owner("v1host-kill-dead2", "owner");
        assert_eq!(
            handle_v1_host_kill(&ungranted, "v1host-kill", "v1host-kill-dead2").status,
            "404 Not Found",
        );
    }

    /// Live kill must keep the durable api-% row so dead-resume can find the
    /// conversation id (preserve_index path). not_live orphan sweeps still clear.
    #[test]
    fn live_kill_preserve_index_leaves_durable_row() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-kill-preserve";
        let pid = insert_project("v1host-kill-preserve", ws);
        let sid = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
        insert_host_tab_session(
            &pid,
            sid,
            "api-owner-bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
        );
        assert!(host_session_resumable(ws, sid), "precondition: row present");

        // Simulate live-kill teardown without a live PTY: preserve_index only.
        let parsed = SessionId::parse(sid).expect("uuid");
        crate::sandbox_reaper::force_teardown_host_session_preserve_index(&parsed, ws, sid);
        assert!(
            host_session_resumable(ws, sid),
            "live-kill preserve must leave durable row for dead-resume"
        );

        // Grace-class clear still drops the row.
        crate::sandbox_reaper::force_teardown_host_session_ctx(&parsed, ws, sid);
        assert!(
            !host_session_resumable(ws, sid),
            "grace/clear path must drop durable row"
        );
    }

    /// Scout restart case: no in-memory owner map, durable api-* tab row only.
    /// Owner floor clears; exact-key prefix clears; wrong key 404s.
    #[test]
    fn kill_durable_row_without_live_owner_map() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-kill-durable";
        let pid = insert_project("v1host-kill-dur", ws);
        let sid_owner = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let sid_key = "ffffffff-1111-2222-3333-444444444444";
        let sid_other = "55555555-6666-7777-8888-999999999999";
        // Host-minted names: api-{principal}-{uuid}
        insert_host_tab_session(
            &pid,
            sid_owner,
            "api-owner-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        );
        insert_host_tab_session(
            &pid,
            sid_key,
            "api-ec8ba43a-ffffffff-1111-2222-3333-444444444444",
        );
        insert_host_tab_session(
            &pid,
            sid_other,
            "api-otherkey-55555555-6666-7777-8888-999999999999",
        );

        // No record_owner — simulates post-restart empty OWNERS map.
        let r = handle_v1_host_kill(&V1Principal::Owner, "v1host-kill-dur", sid_owner);
        assert_eq!(r.status, "200 OK", "owner floor body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["killed"], false);
        assert_eq!(v["reason"], "not_live");
        assert_eq!(v["indexCleared"], true);

        // Exact API key via durable prefix (workspace grant *).
        let key = apik("ec8ba43a", Some("*"));
        let r = handle_v1_host_kill(&key, "v1host-kill-dur", sid_key);
        assert_eq!(r.status, "200 OK", "key prefix body={}", r.body);

        // Wrong key cannot kill another key's durable row.
        let wrong = apik("wrongkey", Some("*"));
        assert_eq!(
            handle_v1_host_kill(&wrong, "v1host-kill-dur", sid_other).status,
            "404 Not Found",
        );
        // Owner floor can still sweep the other key's durable row.
        let r = handle_v1_host_kill(&V1Principal::Owner, "v1host-kill-dur", sid_other);
        assert_eq!(r.status, "200 OK", "owner sweeps other key body={}", r.body);
    }

    /// messages read: canonical → 404; unowned → 404 via the reused F2 authz;
    /// the owning principal drains its log.
    #[test]
    fn messages_refuses_canonical_and_reuses_owner_authz() {
        k2_core::db::init_for_tests();
        let pid = insert_project("v1host-read", "/tmp/k2-v1host-read");
        insert_canonical_session(&pid, "v1host-read-canon");

        assert_eq!(
            handle_v1_host_messages(&V1Principal::Owner, "v1host-read", "v1host-read-canon", 0)
                .status,
            "404 Not Found",
        );
        assert_eq!(
            handle_v1_host_messages(&V1Principal::Owner, "v1host-read", "v1host-read-unowned", 0)
                .status,
            "404 Not Found",
        );
        // The owner drains its own log (record + append, then read).
        crate::sandbox_responses::record_owner("v1host-read-mine", "owner");
        crate::sandbox_responses::append("v1host-read-mine", "host line".to_string(), true);
        let r = handle_v1_host_messages(&V1Principal::Owner, "v1host-read", "v1host-read-mine", 0);
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        assert!(r.body.contains("host line"), "body={}", r.body);
        assert!(r.body.contains("\"latest_seq\":1"), "body={}", r.body);
        // An ungranted key can't even reach the read (uniform 404).
        let ungranted = apik("k-hr-none", None);
        assert_eq!(
            handle_v1_host_messages(&ungranted, "v1host-read", "v1host-read-mine", 0).status,
            "404 Not Found",
        );
    }

    /// list: authorizes first (ungranted → uniform 404), surfaces only this
    /// workspace's `api-…` rows with a liveness flag.
    #[test]
    fn list_authorizes_then_lists_api_rows_only() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-list";
        let pid = insert_project("v1host-list", ws);
        insert_host_tab_session(&pid, "0a0a0a0a-1b1b-2c2c-3d3d-4e4e4e4e4e4e", "api-owner-xyz");
        insert_host_tab_session(&pid, "5f5f5f5f-6a6a-7b7b-8c8c-9d9d9d9d9d9d", "tab-human-2");

        let r = handle_v1_host_list(&V1Principal::Owner, "v1host-list");
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        let sessions = v["sessions"].as_array().expect("sessions array");
        assert_eq!(sessions.len(), 1, "only api- rows listed; body={}", r.body);
        assert_eq!(sessions[0]["sessionId"], "0a0a0a0a-1b1b-2c2c-3d3d-4e4e4e4e4e4e");
        assert_eq!(sessions[0]["live"], false, "no live PTY in this test");
        assert_eq!(v["workspace"], "v1host-list");

        let ungranted = apik("k-hl-none", None);
        assert_eq!(
            handle_v1_host_list(&ungranted, "v1host-list").status,
            "404 Not Found",
        );
    }

    /// Slice W3 honesty: an api- row with NO stamped session id (a
    /// self-minting provider pre-adoption, or a no-adapter preset forever)
    /// is ABSENT from the list — never a null/garbage entry — and is not
    /// resumable.
    #[test]
    fn list_and_resume_omit_unstamped_rows() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-unstamped";
        let pid = insert_project("v1host-unstamped", ws);
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT OR REPLACE INTO workspace_tab_sessions \
                     (project_id, pane_group_id, agent_name, session_id, command) \
                 VALUES (?1, 'api-owner-null', 'api-owner-null', NULL, 'aider')",
                rusqlite::params![pid],
            )
            .expect("insert NULL-session row");
        }
        let r = handle_v1_host_list(&V1Principal::Owner, "v1host-unstamped");
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(
            v["sessions"].as_array().map(Vec::len),
            Some(0),
            "unstamped rows must be absent, not surfaced as null; body={}",
            r.body
        );
    }

    /// Slice W3: `host_session_resumable` admits a NON-UUID provider-minted
    /// id (hermes-shaped) once adopted — authorization is the workspace's
    /// own api- index, not the id's shape.
    #[test]
    fn host_session_resumable_accepts_non_uuid_adopted_ids() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-nonuuid";
        let pid = insert_project("v1host-nonuuid", ws);
        insert_host_tab_session(&pid, "20260706_090000_abcdef", "api-owner-hermes");
        assert!(host_session_resumable(ws, "20260706_090000_abcdef"));
        assert!(!host_session_resumable(ws, "20990101_000000_zzzzzz"));
    }

    /// Pure phase priority: working > grace > finished > never_started > gone.
    #[test]
    fn status_phase_priority_order() {
        assert_eq!(
            compute_host_status_phase(true, true, true, true, false),
            Some("working"),
            "live always wins"
        );
        assert_eq!(
            compute_host_status_phase(false, true, true, true, false),
            Some("grace"),
            "reaper grace before finished"
        );
        assert_eq!(
            compute_host_status_phase(false, false, true, true, false),
            Some("finished"),
            "started + not live + not grace"
        );
        assert_eq!(
            compute_host_status_phase(false, false, false, true, false),
            Some("never_started"),
            "known durable/owned, no start evidence"
        );
        assert_eq!(
            compute_host_status_phase(false, false, false, false, true),
            Some("gone"),
            "tombstone only"
        );
        assert_eq!(
            compute_host_status_phase(false, false, false, false, false),
            None,
            "unknown → 404"
        );
    }

    /// Julie case: durable row, not live, no transcript file, seq 0 →
    /// `phase=never_started`, `started=false`. Field is `latest_seq` (snake).
    #[test]
    fn status_never_started_durable_dead_no_file_seq0() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-status-ns";
        let pid = insert_project("v1host-status-ns", ws);
        // Julie's session shape (unique id for this test process).
        let sid = "1c21112b-90e0-48eb-bbc9-007f8e588f12";
        insert_host_tab_session(
            &pid,
            sid,
            "api-owner-1c21112b-90e0-48eb-bbc9-007f8e588f12",
        );
        // No record_owner, no respond lines, no live PTY, no provider file.

        let r = handle_v1_host_status(&V1Principal::Owner, "v1host-status-ns", sid);
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["sessionId"], sid);
        assert_eq!(v["workspace"], "v1host-status-ns");
        assert_eq!(
            v["agentName"],
            "api-owner-1c21112b-90e0-48eb-bbc9-007f8e588f12"
        );
        assert_eq!(v["live"], false);
        assert_eq!(v["started"], false, "no live/seq/file → started=false");
        assert_eq!(v["phase"], "never_started");
        assert_eq!(v["latest_seq"], 0, "snake_case field only");
        assert!(
            !r.body.contains("latestSeq"),
            "must not dual-emit camelCase latestSeq; body={}",
            r.body
        );
        assert_eq!(v["reaper"], "none");
        assert_eq!(v["durable"], true);
    }

    /// Unowned / wrong key → uniform 404 (no existence oracle).
    #[test]
    fn status_unowned_uniform_404() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-status-unown";
        let pid = insert_project("v1host-status-unown", ws);
        let sid = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
        insert_host_tab_session(
            &pid,
            sid,
            "api-otherkey-bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
        );

        // Wrong API key (granted on this ws) cannot observe another key's row.
        let wrong = apik("wrongkey", Some("*"));
        assert_eq!(
            handle_v1_host_status(&wrong, "v1host-status-unown", sid).status,
            "404 Not Found",
        );
        // Completely unknown id → same 404.
        assert_eq!(
            handle_v1_host_status(
                &V1Principal::Owner,
                "v1host-status-unown",
                "00000000-0000-4000-8000-000000000099"
            )
            .status,
            "404 Not Found",
        );
        // Canonical → 404.
        insert_canonical_session(&pid, "cccccccc-dddd-eeee-ffff-000000000001");
        assert_eq!(
            handle_v1_host_status(
                &V1Principal::Owner,
                "v1host-status-unown",
                "cccccccc-dddd-eeee-ffff-000000000001"
            )
            .status,
            "404 Not Found",
        );
    }

    /// Owner floor can observe any durable `api-*` row (Scout reconciler).
    #[test]
    fn status_owner_floor_observes_durable_api_row() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-status-owner";
        let pid = insert_project("v1host-status-owner", ws);
        let sid = "dddddddd-eeee-ffff-aaaa-111111111111";
        insert_host_tab_session(
            &pid,
            sid,
            "api-ec8ba43a-dddddddd-eeee-ffff-aaaa-111111111111",
        );

        let r = handle_v1_host_status(&V1Principal::Owner, "v1host-status-owner", sid);
        assert_eq!(r.status, "200 OK", "owner floor body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["phase"], "never_started");
        assert_eq!(v["durable"], true);
        assert_eq!(v["started"], false);
        assert_eq!(v["live"], false);

        // Exact key also observes via durable prefix.
        let key = apik("ec8ba43a", Some("*"));
        let r = handle_v1_host_status(&key, "v1host-status-owner", sid);
        assert_eq!(r.status, "200 OK", "key prefix body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["phase"], "never_started");
    }

    /// `started` true when latest_seq > 0 even if not live → `finished`.
    #[test]
    fn status_finished_when_seq_positive_not_live() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-status-fin";
        let pid = insert_project("v1host-status-fin", ws);
        let sid = "eeeeeeee-ffff-aaaa-bbbb-222222222222";
        insert_host_tab_session(
            &pid,
            sid,
            "api-owner-eeeeeeee-ffff-aaaa-bbbb-222222222222",
        );
        crate::sandbox_responses::record_owner(sid, "owner");
        let seq = crate::sandbox_responses::append(sid, "done".to_string(), true);
        assert!(seq >= 1, "append must assign seq");

        let r = handle_v1_host_status(&V1Principal::Owner, "v1host-status-fin", sid);
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["live"], false);
        assert_eq!(v["started"], true, "seq>0 implies started");
        assert_eq!(v["phase"], "finished");
        assert_eq!(v["latest_seq"], seq);
        assert_eq!(
            crate::sandbox_responses::latest_seq(sid),
            seq,
            "public latest_seq helper must match status field"
        );
        assert!(
            !r.body.contains("latestSeq"),
            "snake_case only; body={}",
            r.body
        );
    }

    /// Live-but-no-seq (pure phase path): `live` forces `working` + `started`.
    /// Product lock: live + seq 0 + no transcript ⇒ NEVER never_started.
    #[test]
    fn status_phase_live_is_working_and_started() {
        // Pure function — no PTY construction required.
        let phase = compute_host_status_phase(
            /*live*/ true,
            /*reaper_grace*/ false,
            /*started*/ true, // live ⇒ started by PRD §5
            /*known*/ true,
            /*tombstone_only*/ false,
        );
        assert_eq!(phase, Some("working"));
        // Even if a caller forgot to set started from live, phase still
        // prioritizes live → working (started flag independent for field).
        assert_eq!(
            compute_host_status_phase(true, false, false, true, false),
            Some("working"),
        );
        // started formula: live || latest_seq > 0 || file_exists
        let live = true;
        let latest_seq: u64 = 0;
        let file_exists = false;
        let started = live || latest_seq > 0 || file_exists;
        assert!(started, "live alone is started evidence");
        assert_ne!(
            compute_host_status_phase(live, false, started, true, false),
            Some("never_started"),
            "live+seq0+no-file must never be never_started"
        );
    }

    /// Status is read-only: no kill tombstone, no reaper registration, no
    /// durable row mutation, no respond append.
    #[test]
    fn status_has_no_side_effects() {
        k2_core::db::init_for_tests();
        let ws = "/tmp/k2-v1host-status-se";
        let pid = insert_project("v1host-status-se", ws);
        let sid = "ffffffff-aaaa-bbbb-cccc-333333333333";
        let agent = "api-owner-ffffffff-aaaa-bbbb-cccc-333333333333";
        insert_host_tab_session(&pid, sid, agent);

        let before_seq = crate::sandbox_responses::latest_seq(sid);
        assert_eq!(before_seq, 0);
        assert!(kill_tombstone_owner(sid).is_none());
        assert!(host_session_resumable(ws, sid));

        let r = handle_v1_host_status(&V1Principal::Owner, "v1host-status-se", sid);
        assert_eq!(r.status, "200 OK", "body={}", r.body);
        assert_eq!(
            handle_v1_host_status(&V1Principal::Owner, "v1host-status-se", sid).status,
            "200 OK",
            "idempotent re-read"
        );

        // No tombstone stamped.
        assert!(
            kill_tombstone_owner(sid).is_none(),
            "status must not write kill tombstone"
        );
        // No respond drain growth.
        assert_eq!(
            crate::sandbox_responses::latest_seq(sid),
            0,
            "status must not append respond lines"
        );
        // Durable row still present (not cleared).
        assert!(
            host_session_resumable(ws, sid),
            "status must not clear durable api-% rows"
        );
        // Reaper not registered just by observing.
        if let Some(parsed) = SessionId::parse(sid) {
            assert!(
                !crate::sandbox_reaper::registered(&parsed),
                "status must not register reaper"
            );
            assert_eq!(crate::sandbox_reaper::phase_label(&parsed), None);
        }
    }
}
