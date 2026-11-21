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
//!
//! **`kind: "postal"`** (prd-postal-in-k2-v1 S0, P15–P18, P21): p5's knock
//! for a Postal message. Accepted only from the p5 link key K2 made for this
//! workspace (label `postal-link:<ws>`); `from` must be a Postal address;
//! K2 writes the `[postal <from> · m_…]` label on its own line and quotes
//! every body line; never a command; one delivery per `idempotency_key`
//! (the Postal message id) for that key.

use crate::cli_response::CliResponse;
use crate::routes::http::{V1Capability, V1Principal};

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

/// A31 (prd-daemon-activity-and-thread-working-v1): whether a display
/// refuses a `/v1` message, and the old word it is reported as. Busy =
/// `working` or `waiting` only; `monitoring` (lead idle, background work),
/// `unverifiable` and `idle` accept the message, so integrators see no
/// more refusals than the old hook-only gate gave them.
pub fn busy_word(display: k2_core::activity::Display) -> Option<&'static str> {
    use k2_core::activity::Display;
    match display {
        Display::Working | Display::Waiting => Some(crate::activity_events::legacy_word(display)),
        Display::Monitoring | Display::Unverifiable | Display::Idle => None,
    }
}

/// `working` | `permission` when a live session at `project_path` is busy
/// (its activity-store row, the SAME rows the ops overview reads), else
/// `None`.
pub fn busy_status_for_path(project_path: &str) -> Option<&'static str> {
    let displays = crate::activity_events::displays();
    crate::v2_session_map::snapshot().into_iter().find_map(|(_addr, session)| {
        let matches = session
            .cwd
            .as_ref()
            .is_some_and(|p| p.to_string_lossy() == project_path);
        if !matches {
            return None;
        }
        displays
            .get(&session.session_id.to_string())
            .and_then(|(d, _)| busy_word(*d))
    })
}

/// Gates (2) and (3): `Some(answer)` when the message must not be typed in.
pub(crate) fn consent_and_busy(project_path: &str) -> Option<CliResponse> {
    // (2) CONSENT — per-workspace remote-instruct opt-in (same gate as federation).
    if !k2_core::workspace::settings::remote_instruct_allowed_for_path(project_path) {
        return Some(CliResponse::ok_json(
            serde_json::json!({
                "delivered": false,
                "mode": "declined",
                "reason": "remote_instruction_disabled",
            })
            .to_string(),
        ));
    }
    // (3) BUSY — refuse to inject into a working / HITL-permission agent.
    busy_status_for_path(project_path).map(|status| CliResponse {
        status: "409 Conflict",
        content_type: "application/json",
        body: serde_json::json!({
            "delivered": false,
            "reason": "agent_busy",
            "agentStatus": status,
        })
        .to_string(),
    })
}

pub(crate) fn handle_v1_ws_message(principal: &V1Principal, ws_raw: &str, body: &[u8]) -> CliResponse {
    // Phase 0 — canonical-message capability (owner always has it). Missing
    // cap → uniform 404 (same as ungranted workspace; no existence oracle).
    if !principal.has_capability(V1Capability::CanonicalMessage) {
        return crate::v1_sandboxes::uniform_ws_404();
    }
    let Some(slug) = decode_ws_segment(ws_raw) else {
        return CliResponse::not_found();
    };

    // (1) AUTH — resolve the slug to its registered path + enforce the key's grant.
    let project_path = match crate::v1_sandboxes::resolve_authorized_workspace_door(
        principal,
        &slug,
        k2_core::hire::limits::ReachDoor::Message,
    ) {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    // Parse the body: text (required), from (optional), wake (optional, def true).
    let v: serde_json::Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return CliResponse::bad_request("invalid JSON body"),
    };
    if v.get("kind").and_then(|x| x.as_str()).map(str::trim) == Some("postal") {
        return crate::v1_ws_postal::handle(principal, &project_path, &v);
    }
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

    if let Some(refused) = consent_and_busy(&project_path) {
        return refused;
    }

    // Deliver via the SAME engine as `k2 talk` (wake=true → live-inject or
    // wake+resume+inject a dormant canonical session).
    // S7: recorded, so the recipient can check the sender (`k2 msg who`).
    let on_behalf_of = v
        .get("onBehalfOf")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().chars().take(200).collect::<String>())
        .filter(|s| !s.is_empty());
    let sender = match principal {
        V1Principal::Owner => crate::hire_routes::Sender {
            workspace_id: None,
            via: "owner",
            actor: "owner-token".into(),
            on_behalf_of: None,
        },
        V1Principal::Api(p) => crate::hire_routes::Sender {
            workspace_id: None,
            via: "api-key",
            actor: format!("api-key:{}", p.id),
            on_behalf_of,
        },
    };
    let (result, delivery) = crate::hire_routes::deliver_recorded(
        &project_path,
        &text,
        &from,
        "",
        wake,
        crate::workspace_msg::DEFAULT_WAKE_TIMEOUT,
        "v1",
        &sender,
    );
    CliResponse::ok_json(
        serde_json::json!({
            "delivered": result.success,
            "mode": "live",
            "result": public_result(&result),
            "deliveryId": delivery,
        })
        .to_string(),
    )
}

/// A delivery result as a `/v1` caller sees it: without
/// `target_session_id`, the daemon id of the agent's live terminal (0.46.17).
/// An API key never needs it, and no `/v1` door takes it.
pub(crate) fn public_result(result: &crate::workspace_msg::MsgResponse) -> serde_json::Value {
    let mut v = serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(o) = v.as_object_mut() {
        o.remove("target_session_id");
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::http::V1Principal;

    /// T-S4g (A31): `monitoring` accepts a message, `working` and
    /// `waiting` refuse it with the old word.
    /// `/v1` delivery answers never carry the agent terminal's daemon id.
    #[test]
    fn public_result_drops_the_terminal_id() {
        let mut r = crate::workspace_msg::MsgResponse::fail(crate::workspace_msg::MsgReason::WorkspaceNotFound);
        r.success = true;
        r.target_session_id = Some("11111111-2222-3333-4444-555555555555".to_string());
        let v = public_result(&r);
        assert!(v.get("target_session_id").is_none(), "{v}");
        assert!(!v.to_string().contains("11111111-2222-3333-4444-555555555555"), "{v}");
        assert_eq!(v["success"], serde_json::json!(true));
    }

    #[test]
    fn busy_is_working_or_waiting_only() {
        use k2_core::activity::Display;
        assert_eq!(busy_word(Display::Working), Some("working"));
        assert_eq!(busy_word(Display::Waiting), Some("permission"));
        assert_eq!(busy_word(Display::Monitoring), None);
        assert_eq!(busy_word(Display::Unverifiable), None);
        assert_eq!(busy_word(Display::Idle), None);
    }

    /// Phase 0 — missing `canonical_message` capability → uniform 404.
    #[test]
    fn canonical_message_requires_capability() {
        k2_core::db::init_for_tests();
        // Register a workspace so a full-cap key would get past the grant.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![
                    uuid::Uuid::new_v4().to_string(),
                    "v1msg-cap-deny",
                    "/tmp/k2-v1msg-cap-deny",
                ],
            )
            .expect("insert project");
        }
        let no_canon = V1Principal::Api(k2_core::api_keys::ApiPrincipal {
            id: "k-msg-no-cap".to_string(),
            anthropic_key: None,
            provider: None,
            base_url: None,
            scope: "owner".to_string(),
            allowed_workspaces: Some("*".to_string()),
            capabilities: k2_core::api_keys::ApiCapabilities::new_key_default(), // canon OFF
        });
        let r = handle_v1_ws_message(
            &no_canon,
            "v1msg-cap-deny",
            br#"{"text":"hello"}"#,
        );
        assert_eq!(r.status, "404 Not Found", "body={}", r.body);
        assert!(r.body.contains("no such workspace"), "uniform body: {}", r.body);
    }
}
