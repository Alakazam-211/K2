//! `POST /v1/w/<ws>/message` — message a workspace's CANONICAL agent session via
//! the API (the "workspace IS an agent" payoff: drive the real agent people use,
//! not a sandbox cell). Reuses the exact `k2 talk` engine
//! ([`workspace_msg::deliver_live`], wake=true): inject if the canonical session
//! is live, wake+resume+inject if dormant.
//!
//! Three gates, in order:
//! 1. **Auth** — V1 principal must be authorized for `<ws>` (per-key grant).
//! 2. **Consent** — the per-workspace remote-instruct opt-in (the SAME gate
//!    federation uses for canonical-chat drive). OFF → `declined`.
//! 3. **Busy** — never inject into an agent that is mid-work (`working`) or
//!    sitting on a HITL permission prompt (`permission`), since the injected
//!    text could be consumed as the prompt's answer. Busy → `409 agent_busy`.

use crate::cli_response::CliResponse;
use crate::routes::http::V1Principal;

/// Percent-decode + validate a `<ws>` URL segment (reject empty/slash/traversal).
/// Mirrors `v1_sandboxes::decode_and_validate_segment` (private there).
fn decode_ws_segment(raw: &str) -> Option<String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |c: u8| (c as char).to_digit(16);
            if let (Some(a), Some(b)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((a * 16 + b) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let s = String::from_utf8(out).ok()?;
    if s.is_empty() || s.contains('/') || s.contains("..") {
        return None;
    }
    Some(s)
}

/// The canonical agent's current hook status for a live session at `project_path`
/// (`working`|`idle`|`permission`|…), or `None` when no live session / no status
/// observed. Reads the SAME cached `AgentStatusChanged` the ops overview uses.
fn canonical_status_for_path(project_path: &str) -> Option<String> {
    for (_addr, session) in crate::v2_session_map::snapshot() {
        let matches = session
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy() == project_path)
            .unwrap_or(false);
        if matches {
            if let Some((raw, _ts)) =
                crate::session_events::agent_status_for(&session.session_id.to_string())
            {
                return Some(match raw.as_str() {
                    "start" => "working".to_string(),
                    "stop" => "idle".to_string(),
                    other => other.to_string(),
                });
            }
        }
    }
    None
}

pub(crate) fn handle_v1_ws_message(principal: &V1Principal, ws_raw: &str, body: &[u8]) -> CliResponse {
    let Some(slug) = decode_ws_segment(ws_raw) else {
        return CliResponse::not_found();
    };

    // (1) AUTH — resolve the slug to its registered path + enforce the key's grant.
    let project_path = match crate::v1_sandboxes::resolve_authorized_workspace(principal, &slug) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Parse the body: text (required), from (optional), wake (optional, def true).
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return CliResponse::bad_request("invalid JSON body"),
    };
    let text = v
        .get("text")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if text.is_empty() {
        return CliResponse::bad_request("Missing 'text'");
    }
    let from = v
        .get("from")
        .and_then(|x| x.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(crate::workspace_msg::resolve_owner_from);
    let wake = v.get("wake").and_then(|x| x.as_bool()).unwrap_or(true);

    // (2) CONSENT — per-workspace remote-instruct opt-in (same gate as federation).
    if !k2_core::workspace::settings::remote_instruct_allowed_for_path(&project_path) {
        return CliResponse::ok_json(
            serde_json::json!({
                "delivered": false,
                "mode": "declined",
                "reason": "remote_instruction_disabled",
            })
            .to_string(),
        );
    }

    // (3) BUSY — refuse to inject into a working / HITL-permission agent.
    if let Some(status) = canonical_status_for_path(&project_path) {
        if status == "working" || status == "permission" {
            return CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body: serde_json::json!({
                    "delivered": false,
                    "reason": "agent_busy",
                    "agentStatus": status,
                })
                .to_string(),
            };
        }
    }

    // Deliver via the SAME engine as `k2 talk` (wake=true → live-inject or
    // wake+resume+inject a dormant canonical session).
    let result = crate::workspace_msg::deliver_live(
        &project_path,
        &text,
        &from,
        "",
        wake,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
    );
    CliResponse::ok_json(
        serde_json::json!({
            "delivered": result.success,
            "mode": "live",
            "result": result,
        })
        .to_string(),
    )
}
