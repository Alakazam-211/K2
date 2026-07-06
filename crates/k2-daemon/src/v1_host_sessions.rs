//! F1 (prd-v1-api-completion §3) — NON-SANDBOXED **HOST SESSIONS** on `/v1`:
//! the sibling route family that makes a sandbox-less host (K2 Cloud
//! Standard, shared VPS, Raspberry Pi, any Mac) a first-class API citizen.
//!
//! ```text
//! POST /v1/w/<ws>/host-sessions              → spawn (or resume with {"session": id})
//! GET  /v1/w/<ws>/host-sessions              → list this ws's api-spawned host sessions
//! POST /v1/w/<ws>/host-sessions/<id>         → message-live (inject into the PTY)
//! GET  /v1/w/<ws>/host-sessions/<id>/messages?since=<seq>
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
use crate::routes::http::V1Principal;
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
/// advertise readiness — mirrors the wake path's test seam.
const SPAWN_PROMPT_READY_TIMEOUT_SECS: u64 = 15;

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
pub const API_SPAWN_PREAMBLE: &str = "[K2 API] You were invoked through the K2 API. Report progress and results back to the caller by running: k2 respond '<your message>' — and send your final answer with: k2 respond --final '<your answer>'. The caller cannot see your terminal; only k2 respond output reaches them.";

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
/// Scoped to THIS workspace's `api-…` rows on the adopted path (same index
/// as [`host_session_resumable`]); the direct path's workspace pin is
/// enforced by the callers' existing cwd checks / resume-door index check.
fn lookup_live_host_session(
    ws_path: &str,
    session_id: &str,
) -> Option<std::sync::Arc<k2_core::terminal::DaemonPtySession>> {
    if let Some(sid) = SessionId::parse(session_id) {
        if let Some(live) = crate::v2_session_map::lookup_by_session_id(&sid) {
            return Some(live);
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
    agents
        .iter()
        .find_map(|a| crate::v2_session_map::lookup_by_agent_name(a))
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
fn deliver_into_live(sid: &SessionId, echo_sid: &str, prompt: &str) -> CliResponse {
    crate::sandbox_reaper::stamp(sid);
    let delivered = if prompt.is_empty() {
        // Nothing to inject — the touch (reaper re-arm) is the whole effect.
        true
    } else {
        crate::workspace_msg::inject_raw_into_session(sid, prompt, Duration::ZERO)
    };
    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": echo_sid,
            "delivered": delivered,
            "live": true,
        })
        .to_string(),
    )
}

/// `POST /v1/w/<ws>/host-sessions` — spawn a fresh NON-SANDBOXED host session
/// in the granted workspace (or RESUME one of this family's own prior
/// sessions when the body carries `{"session": <id>}`).
pub fn handle_v1_host_new(principal: &V1Principal, ws_raw: &str, body: &[u8]) -> CliResponse {
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
    // `lookup_live_host_session` covers both id shapes: the daemon SessionId
    // (premint providers) and an ADOPTED provider-minted id (self-minting
    // providers, routed via the api- row's agent_name).
    if let Some(target) = resume_target.as_deref() {
        if let Some(live) = lookup_live_host_session(&ws_path, target) {
            let prompt = req.prompt.as_deref().unwrap_or("").trim().to_string();
            return deliver_into_live(&live.session_id, target, &prompt);
        }
    }

    // CONCURRENT-SESSION CAP — the same per-principal + global quota the
    // sandbox doors use (PRD §3: reused as-is). Acquire BEFORE any side
    // effect; released by the child-exit observer on success, or explicitly
    // on the early-failure paths below.
    let principal_key = principal.display_id();
    if let Err(qe) = sandbox_quota::try_acquire(&principal_key) {
        return CliResponse {
            status: "429 Too Many Requests",
            content_type: "application/json",
            body: serde_json::json!({ "error": qe.message(), "code": qe.code() }).to_string(),
        };
    }

    // The forced daemon SessionId: a UUID-shaped resume target rides it
    // (premint continuity — returned id == conversation id); a non-UUID
    // provider-minted target (hermes) can't, so the PTY gets a fresh
    // daemon id while the RESUME GRAMMAR below still carries the target
    // (policy splices `req.session`, and registration re-stamps the new
    // `api-…` row with it via the table-driven argv scan).
    let is_resume = resume_target.is_some();
    let session_id = resume_target
        .as_deref()
        .and_then(SessionId::parse)
        .unwrap_or_else(SessionId::new);

    // The PASSTHROUGH POLICY RESOLVER (the one new security-critical piece):
    // cwd pinned, command host-minted from the workspace's configured agent,
    // caller env/args dropped, principal key staged, danger flags stripped
    // unless the owner opted in.
    let mut spawn_req =
        policy::resolve_host_spawn(principal, &ws_path, &session_id, is_resume, &req);
    spawn_req.principal_key = Some(principal_key.clone());
    // Captured for the post-spawn adoption decision below (spawn_session
    // consumes the request).
    let spawn_command = spawn_req.command.clone().unwrap_or_default();

    // Idle-reap timeout — the caller's knob, identical semantics to the
    // sandbox family (clamped 30..86400, default 180).
    let timeout_secs = crate::sandbox_reaper::normalize_timeout(req.timeout_secs);

    // Spawn through the PROVEN v2 internals (find-or-spawn can't collide —
    // the agent_name is a fresh host-minted `api-…` uuid).
    let result = v2_spawn::spawn_session(spawn_req);
    if result.status != "200 OK" {
        // No session ⇒ no child-exit observer ⇒ release the slot ourselves.
        sandbox_quota::release(&principal_key);
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
                    ws_path.clone(),
                    agent_name.to_string(),
                    principal_key.clone(),
                );
            }
        }
    }

    // Initial prompt: host sessions have no guest-init to read a staged env
    // var, so deliver it into the PTY once the TUI is ready — on a DETACHED
    // thread (readiness polling + the locked injector sleep ~½s+; the API
    // response must not wait). Best-effort by design: the caller confirms
    // via `GET .../messages` / the grid stream. Value never logged.
    //
    // The INITIAL prompt is wrapped with [`API_SPAWN_PREAMBLE`] — this is
    // the fresh process's one and only briefing on the `k2 respond`
    // read-back contract. Both paths through here are fresh processes
    // (plain spawn, and resume-of-a-DEAD-session which re-spawns), so both
    // carry it; follow-ups via [`deliver_into_live`] stay raw.
    let prompt = req.prompt.as_deref().unwrap_or("").trim().to_string();
    if !prompt.is_empty() {
        let payload = format!("{API_SPAWN_PREAMBLE}\n\n{prompt}");
        let sid_for_inject = sid;
        let ready_timeout = spawn_prompt_ready_timeout();
        // W4: the spawned agent's readiness dialect via the ONE shared
        // precedence chain (preset-declared `readiness` metadata → static
        // provider table → poll default). ?2004h LIES for codex/gemini/
        // pi/hermes — polling bracketed paste would inject into their
        // startup dialogs (which EAT text); those profiles wait their
        // settle floor instead, still capped by the ceiling above.
        let inject_profile = policy::resolve_host_injection_profile(&ws_path);
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
    }

    // Per-session STREAM token (grid WS) — the caller streams with this,
    // never the API key.
    let stream_tok = stream_token::mint(&sid);
    let grid = format!("/cli/sessions/grid?session={session_id_str}&token={stream_tok}");

    // FROZEN spawn wire shape (PRD §3): exactly these five keys, with the
    // honest `"sandbox":"none"` label.
    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": session_id_str,
            "agentName": agent_name,
            "workspace": slug,
            "sandbox": "none",
            "stream": { "grid": grid },
        })
        .to_string(),
    )
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
pub fn handle_v1_host_list(principal: &V1Principal, ws_raw: &str) -> CliResponse {
    let Some(slug) = decode_and_validate_segment(ws_raw) else {
        return uniform_ws_404();
    };
    // Authz FIRST — an unauthorized caller must not even learn the ws exists.
    let ws_path = match resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let rows: Vec<(String, String, i64)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let queried: Result<Vec<(String, String, i64)>, rusqlite::Error> = (|| {
            let mut stmt = conn.prepare(
                "SELECT wts.session_id, wts.agent_name, wts.last_seen_at \
                 FROM workspace_tab_sessions wts \
                 JOIN projects p ON p.id = wts.project_id \
                 WHERE p.path = ?1 AND wts.agent_name LIKE 'api-%' \
                   AND wts.session_id IS NOT NULL \
                 ORDER BY wts.last_seen_at DESC",
            )?;
            let mapped = stmt.query_map(rusqlite::params![ws_path], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })?;
            Ok(mapped.flatten().collect())
        })();
        match queried {
            Ok(v) => v,
            Err(e) => return CliResponse::internal_error(format!("list host sessions: {e}")),
        }
    };

    let sessions: Vec<serde_json::Value> = rows
        .iter()
        .map(|(sid, agent, last_seen)| {
            // Liveness: the `api-…` agent_name IS the v2 map's registration
            // key, so it resolves for BOTH id shapes — including adopted
            // provider-minted ids, whose live PTY runs under a different
            // (forced daemon) SessionId. The SessionId lookup stays as a
            // fallback for a row whose PTY outlived its map key shape.
            let live = crate::v2_session_map::lookup_by_agent_name(agent).is_some()
                || SessionId::parse(sid)
                    .and_then(|s| crate::v2_session_map::lookup_by_session_id(&s))
                    .is_some();
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

/// `POST /v1/w/<ws>/host-sessions/<id>` — MESSAGE-LIVE: inject the caller's
/// prompt into a LIVE host session's PTY. A dead/unknown/unowned/canonical id
/// is a uniform 404 (resume rides the spawn route's `{"session": id}`).
pub fn handle_v1_host_message(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
    body: &[u8],
) -> CliResponse {
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

    let prompt = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("prompt").and_then(|x| x.as_str()).map(str::to_string))
        .unwrap_or_default();
    deliver_into_live(&live.session_id, &sid_seg, prompt.trim())
}

/// `GET /v1/w/<ws>/host-sessions/<id>/messages?since=<seq>` — drain the
/// in-session agent's `k2 respond` log. Authorizes the workspace, refuses the
/// canonical session, then reuses the F2 drain
/// ([`crate::v1_sandboxes::handle_messages`]) whose default-deny owner check
/// still applies — a workspace-authorized caller reads only sessions it OWNS.
pub fn handle_v1_host_messages(
    principal: &V1Principal,
    ws_raw: &str,
    sid_raw: &str,
    since: u64,
) -> CliResponse {
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

#[cfg(test)]
mod tests {
    use super::*;

    // Shared in-memory test DB (process-global) — unique `v1host-` prefixes
    // keep these rows disjoint from every other module's tests.

    fn apik(id: &str, grant: Option<&str>) -> V1Principal {
        V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: id.to_string(),
            anthropic_key: None,
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: grant.map(str::to_string),
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
}
