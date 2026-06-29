//! `POST /v1/sandboxes` — the PUBLIC sandbox-spawn route (P3b).
//!
//! The external "K2-as-a-server" front door: an authenticated [`V1Principal`]
//! (owner token or a valid API key — resolved by `routes::http::v1_principal`)
//! asks for a fresh isolated agent session. The whole `/v1/*` surface sits
//! behind `misc_routes::sandbox_api_enabled()` (`K2_SANDBOX_API`, default OFF),
//! so this route is DARK / 404 unless explicitly enabled.
//!
//! Flow:
//!   1. **REFUSE if we can't sandbox** — INVERT v2/spawn's degrade-to-passthrough:
//!      a public sandbox API must NEVER silently run unsandboxed. If
//!      `v2_spawn::can_sandbox()` is false (every macOS / feature-off build) →
//!      `409 Conflict`. The real path is on-box (Linux + `sandbox-microvm` +
//!      `K2_SANDBOX`).
//!   2. **Policy-resolve** the untrusted body into a host-trusted `SpawnRequest`
//!      ([`policy::resolve_spawn`] — the AUTHZ DOOR; ephemeral cwd, host-minted
//!      name, dropped caller env/args, principal-staged key).
//!   3. **Spawn** via the PROVEN `v2_spawn::spawn_session` internals.
//!   4. **Mint a per-session stream token** (`stream_token`) and return the
//!      grid/bytes stream URLs scoped to THIS session — the API key is never
//!      broadened to the raw PTY streams.

pub mod policy;

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;
use crate::{stream_token, v2_spawn};

use policy::{resolve_spawn, ApiSandboxRequest};

/// Handle `POST /v1/sandboxes`. The caller is ALREADY authenticated to a
/// [`V1Principal`] by the dispatcher's `/v1/*` gate; this never re-checks creds,
/// it resolves them into a host-trusted spawn.
pub fn handle_v1_sandboxes(principal: &V1Principal, body: &[u8]) -> CliResponse {
    // (1) REFUSE if this daemon cannot deliver a real microVM. A public sandbox
    // API must NEVER degrade to an unsandboxed passthrough cell — the inverse of
    // v2/spawn's degrade-loud behavior. On Mac / feature-off this ALWAYS 409s.
    if !v2_spawn::can_sandbox() {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: r#"{"error":"this daemon cannot sandbox (microVM backend unavailable)"}"#
                .to_string(),
        };
    }

    // (2) Parse the UNTRUSTED body. Empty/whitespace body → all defaults.
    let api_req: ApiSandboxRequest = if body.iter().all(|b| b.is_ascii_whitespace()) {
        ApiSandboxRequest::default()
    } else {
        match serde_json::from_slice(body) {
            Ok(r) => r,
            Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
        }
    };

    // (3) Run the POLICY-RESOLVER → host-trusted SpawnRequest. Fail-CLOSED: a
    // provisioning failure 5xxs (NEVER a $HOME fallback).
    let spawn_req = match resolve_spawn(principal, &api_req) {
        Ok(r) => r,
        Err(e) => {
            return CliResponse {
                status: e.status(),
                content_type: "application/json",
                body: serde_json::json!({ "error": e.message() }).to_string(),
            }
        }
    };
    // Keep a handle to the ephemeral dir so we can clean up if the spawn itself
    // fails (no session ⇒ no child-exit observer ⇒ no automatic teardown).
    let ephemeral_cwd = spawn_req.ephemeral_cwd.clone();

    // (4) Spawn through the PROVEN v2 internals (inherits B3a key-inject, scoped
    // token/UDS, child-exit teardown incl. the ephemeral-dir rm).
    let result = v2_spawn::spawn_session(spawn_req);
    if result.status != "200 OK" {
        cleanup_ephemeral(&ephemeral_cwd);
        // Surface the spawn's own status/body (already a JSON error envelope).
        return CliResponse {
            status: result.status,
            content_type: "application/json",
            body: result.body,
        };
    }

    // (5) Parse the spawn response for the new session id + agent name.
    let v: serde_json::Value = match serde_json::from_str(&result.body) {
        Ok(v) => v,
        Err(_) => {
            cleanup_ephemeral(&ephemeral_cwd);
            return CliResponse::internal_error("spawn response was not JSON");
        }
    };
    let session_id = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("");
    let agent_name = v.get("agentName").and_then(|x| x.as_str()).unwrap_or("");
    if session_id.is_empty() {
        cleanup_ephemeral(&ephemeral_cwd);
        return CliResponse::internal_error("spawn response carried no sessionId");
    }
    let Some(sid) = k2_core::session::SessionId::parse(session_id) else {
        cleanup_ephemeral(&ephemeral_cwd);
        return CliResponse::internal_error("spawn response sessionId was malformed");
    };

    // (6) Mint a per-session STREAM token (bound to THIS session only) and build
    // the grid/bytes stream URLs. The caller streams with this token — NEVER the
    // API key, which is never accepted on grid/bytes.
    let stream_tok = stream_token::mint(&sid);
    let grid = format!("/cli/sessions/grid?session={session_id}&token={stream_tok}");
    let bytes = format!("/cli/sessions/bytes?session={session_id}&token={stream_tok}");

    CliResponse::ok_json(
        serde_json::json!({
            "sessionId": session_id,
            "agentName": agent_name,
            // can_sandbox() was true above ⇒ resolve_sandbox(true) is Microvm.
            "sandbox": "microvm",
            "stream": { "grid": grid, "bytes": bytes },
        })
        .to_string(),
    )
}

/// Best-effort removal of a half-provisioned ephemeral dir on an early-exit path
/// (spawn failure / malformed response) where no child-exit observer will fire.
fn cleanup_ephemeral(dir: &Option<std::path::PathBuf>) {
    if let Some(d) = dir.as_ref() {
        let _ = std::fs::remove_dir_all(d);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On this (macOS / feature-off) build `can_sandbox()` is false, so the
    /// route MUST 409 BEFORE provisioning anything — never degrading to an
    /// unsandboxed passthrough cell. This is the load-bearing refuse-if-can't
    /// invariant. (On the box with K2_SANDBOX the spawn path is exercised by the
    /// on-box test; it can't run on Mac because the microVM backend is absent.)
    #[test]
    fn refuses_409_when_daemon_cannot_sandbox() {
        // Guard: this assertion is only meaningful where can_sandbox() is false
        // — which is every build this test suite runs on (macOS / feature-off).
        assert!(
            !v2_spawn::can_sandbox(),
            "test premise: this build cannot sandbox",
        );
        let principal = V1Principal::Owner;
        let resp = handle_v1_sandboxes(&principal, b"{}");
        assert_eq!(resp.status, "409 Conflict", "body={}", resp.body);
        assert!(
            resp.body.contains("cannot sandbox"),
            "409 must explain the refusal; body={}",
            resp.body
        );
    }

    /// The 409 fires for an API-key principal too (same refuse-if-can't door).
    #[test]
    fn refuses_409_for_api_principal_too() {
        let principal = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "key-x".to_string(),
            anthropic_key: Some("sk-ant-x".to_string()),
            scope: "owner".to_string(),
        });
        let resp = handle_v1_sandboxes(&principal, b"");
        assert_eq!(resp.status, "409 Conflict");
    }
}
