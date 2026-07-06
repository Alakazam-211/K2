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
fn deliver_into_live(sid: &SessionId, prompt: &str) -> CliResponse {
    crate::sandbox_reaper::stamp(sid);
    let delivered = if prompt.is_empty() {
        // Nothing to inject — the touch (reaper re-arm) is the whole effect.
        true
    } else {
        crate::workspace_msg::inject_raw_into_session(sid, prompt, Duration::ZERO)
    };
    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": sid.to_string(),
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
    // specific state; never silently downgrade to a fresh session).
    let resume_sid: Option<SessionId> = match req.session.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => {
            let Some(validated) = decode_and_validate_segment(raw) else {
                return uniform_ws_404();
            };
            if session_is_canonical(&validated) {
                return uniform_ws_404();
            }
            let Some(parsed) = SessionId::parse(&validated) else {
                return uniform_ws_404();
            };
            if !host_session_resumable(&ws_path, &validated) {
                return uniform_ws_404();
            }
            Some(parsed)
        }
    };

    // Resume of a session that is STILL LIVE = deliver into it (never boot a
    // duplicate PTY under the same forced id) — the F3 liveness-router shape.
    if let Some(sid) = resume_sid.as_ref() {
        if crate::v2_session_map::lookup_by_session_id(sid).is_some() {
            let prompt = req.prompt.as_deref().unwrap_or("").trim().to_string();
            return deliver_into_live(sid, &prompt);
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

    let is_resume = resume_sid.is_some();
    let session_id = resume_sid.unwrap_or_else(SessionId::new);

    // The PASSTHROUGH POLICY RESOLVER (the one new security-critical piece):
    // cwd pinned, command host-minted from the workspace's configured agent,
    // caller env/args dropped, principal key staged, danger flags stripped
    // unless the owner opted in.
    let mut spawn_req =
        policy::resolve_host_spawn(principal, &ws_path, &session_id, is_resume, &req);
    spawn_req.principal_key = Some(principal_key.clone());

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

    // Initial prompt: host sessions have no guest-init to read a staged env
    // var, so deliver it into the PTY once the TUI is ready — on a DETACHED
    // thread (readiness polling + the locked injector sleep ~½s+; the API
    // response must not wait). Best-effort by design: the caller confirms
    // via `GET .../messages` / the grid stream. Value never logged.
    let prompt = req.prompt.as_deref().unwrap_or("").trim().to_string();
    if !prompt.is_empty() {
        let sid_for_inject = sid;
        let ready_timeout = spawn_prompt_ready_timeout();
        std::thread::spawn(move || {
            let ok = crate::workspace_msg::inject_raw_into_session(
                &sid_for_inject,
                &prompt,
                ready_timeout,
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
            let live = SessionId::parse(sid)
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
    let Some(sid) = SessionId::parse(&sid_seg) else {
        return uniform_ws_404();
    };
    // LIVENESS + WORKSPACE PIN: the session must be live AND rooted in THIS
    // workspace (a principal granted two workspaces can't cross-address).
    let Some(live) = crate::v2_session_map::lookup_by_session_id(&sid) else {
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
    deliver_into_live(&sid, prompt.trim())
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
}
