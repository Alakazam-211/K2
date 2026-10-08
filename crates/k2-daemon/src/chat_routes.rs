//! Daemon-side `/cli/chat/*` route handlers (Phase 2 Unit 6).
//!
//! Wraps `k2_core::chat_history::*` for the renderer's chat-history
//! sidebar. Response shapes mirror the pre-Phase-2 Tauri commands
//! byte-for-byte so the renderer can swap endpoints without changing
//! any consumer code.

use std::collections::HashMap;

use serde::Deserialize;

use crate::cli_response::CliResponse;
use k2_core::chat_history as ch;

// ── GET handlers ──────────────────────────────────────────────────────

pub fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let project_filter = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .map(String::as_str);
    match ch::list_all_sessions(project_filter) {
        Ok(sessions) => CliResponse::ok_json(
            serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_storage_paths(params: &HashMap<String, String>) -> CliResponse {
    let project_path = match params
        .get("project_path")
        .or_else(|| params.get("project"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing project_path parameter"),
    };
    match ch::get_storage_paths(&project_path) {
        Ok(paths) => CliResponse::ok_json(
            serde_json::to_string(&paths).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_custom_names(_params: &HashMap<String, String>) -> CliResponse {
    match ch::get_custom_names() {
        Ok(map) => CliResponse::ok_json(
            serde_json::to_string(&map).unwrap_or_else(|_| "{}".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_pinned(_params: &HashMap<String, String>) -> CliResponse {
    match ch::get_pinned() {
        Ok(list) => CliResponse::ok_json(
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_detect_active(params: &HashMap<String, String>) -> CliResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let project_path = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .cloned()
        .unwrap_or_default();
    if provider.is_empty() || project_path.is_empty() {
        return CliResponse::bad_request("Missing 'provider' or 'project_path' parameter");
    }
    match ch::detect_active_session(&provider, &project_path) {
        Ok(opt) => CliResponse::ok_json(
            serde_json::json!({ "sessionId": opt }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_discover_ide(params: &HashMap<String, String>) -> CliResponse {
    let project_path = match params
        .get("project_path")
        .or_else(|| params.get("project"))
        .filter(|s| !s.is_empty())
    {
        Some(p) => p.clone(),
        None => return CliResponse::bad_request("Missing project_path parameter"),
    };
    match ch::discover_ide_sessions(&project_path) {
        Ok(sessions) => CliResponse::ok_json(
            serde_json::to_string(&sessions).unwrap_or_else(|_| "[]".to_string()),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

pub fn handle_session_exists(params: &HashMap<String, String>) -> CliResponse {
    let project_path = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .cloned()
        .unwrap_or_default();
    let session_id = params.get("session_id").cloned().unwrap_or_default();
    if project_path.is_empty() || session_id.is_empty() {
        return CliResponse::bad_request(
            "Missing 'project_path' or 'session_id' parameter",
        );
    }
    let exists = ch::claude_session_file_exists(&session_id, &project_path);
    CliResponse::ok_json(serde_json::json!({ "exists": exists }).to_string())
}

/// GET chat/session-path — resolve the on-disk storage path for a listed chat
/// (transcript / store.db / session dir) for the Chats sidebar "Copy Path".
pub fn handle_session_path(params: &HashMap<String, String>) -> CliResponse {
    let provider = params.get("provider").cloned().unwrap_or_default();
    let session_id = params
        .get("session_id")
        .or_else(|| params.get("sessionId"))
        .cloned()
        .unwrap_or_default();
    let project_path = params
        .get("project_path")
        .or_else(|| params.get("project"))
        .cloned()
        .unwrap_or_default();
    if provider.is_empty() || session_id.is_empty() {
        return CliResponse::bad_request(
            "Missing 'provider' or 'session_id' parameter",
        );
    }
    // Prefer the session's own project field; fall back to list filter path.
    let path = ch::resolve_session_storage_path(&provider, &session_id, &project_path);
    CliResponse::ok_json(
        serde_json::json!({
            "path": path,
            "project": if project_path.is_empty() { serde_json::Value::Null } else { serde_json::json!(project_path) },
        })
        .to_string(),
    )
}

// ── POST handlers ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RenameBody {
    provider: String,
    session_id: String,
    custom_name: String,
    /// TR4: the workspace the app renamed the chat in (TabBar and Chats
    /// both know it). Used before the reverse lookup.
    #[serde(default)]
    project_path: Option<String>,
}

pub fn handle_rename(body: &[u8]) -> CliResponse {
    let parsed: RenameBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match ch::rename_session_scoped(
        &parsed.provider,
        &parsed.session_id,
        &parsed.custom_name,
        parsed.project_path.as_deref(),
    ) {
        Ok(outcome) => {
            // Mirror the Tauri-side `sync:chat-history` event so the
            // renderer's tab list refreshes after a rename. The
            // daemon-side broadcast lands on every `/events` WS
            // subscriber including `daemon_events.rs`, which re-emits
            // onto Tauri's event bus.
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncChatHistory,
                serde_json::Value::Null,
            );
            after_rename(&outcome, &parsed.custom_name);
            let mut body = serde_json::json!({ "success": true });
            if let Some(addr) = outcome.address.as_deref() {
                body["address"] = serde_json::json!(addr);
            }
            if outcome.address_changed() {
                body["previousAddress"] = serde_json::json!(outcome.previous_address);
            }
            CliResponse::ok_json(body.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// TR8: the one line a live session gets when its chat is renamed.
pub fn rename_notice_line(previous: &str, address: &str) -> String {
    format!("[k2] This chat's address is now {address} (was {previous}). Old addresses still reach you.")
}

#[cfg(test)]
static TEST_NOTICES: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

#[cfg(test)]
fn recorded_notices() -> Vec<(String, String)> {
    TEST_NOTICES.lock().map(|g| g.clone()).unwrap_or_default()
}

/// S3: a rename that moved the chat's address tells everyone.
/// - `session_address_changed` on the workspace event socket (TR12).
/// - an `address` frame on the conversation's overlay socket (TR13).
/// - the live session's label, Locked, so a reconnect titles the tab with
///   the new name (Q8, SC33 drift).
/// - a one-line notice into the live session, never a wake (TR8).
fn after_rename(outcome: &ch::RenameOutcome, custom_name: &str) {
    let Some(project_id) = outcome.project_id.as_deref() else {
        return;
    };
    if outcome.canonical {
        return;
    }
    let key = outcome.conversation_key.as_str();
    let live = crate::workspace_msg::lookup_live_for_sidecar(project_id, key);
    let name = custom_name.trim();
    if let Some(live) = live.as_ref() {
        if !name.is_empty() {
            live.0.set_label(name.to_string(), true);
        }
    }
    if !outcome.address_changed() {
        return;
    }
    let (Some(address), Some(previous)) =
        (outcome.address.clone(), outcome.previous_address.clone())
    else {
        return;
    };
    let workspace_path = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![project_id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default()
    };
    // No subscribers is fine (headless, no client attached).
    let _ = crate::session_events::emit(crate::session_events::SessionEvent::SessionAddressChanged {
        workspace_path,
        pane_group_id: crate::workspace_msg::pane_group_for_sidecar(project_id, key),
        conversation_id: key.to_string(),
        address: address.clone(),
        previous_address: previous.clone(),
    });
    crate::overlay_ws::publish_address(key, &address, &previous);
    let Some(live) = live else {
        // Dormant: its next spawn's brief and the next Thread inject carry
        // the new name. Never wake it for this.
        return;
    };
    let line = rename_notice_line(&previous, &address);
    #[cfg(test)]
    if let Ok(mut g) = TEST_NOTICES.lock() {
        g.push((key.to_string(), line.clone()));
    }
    // Off the request thread: the inject waits on the session's lock.
    let spawned = std::thread::Builder::new()
        .name("rename-notice".into())
        .spawn(move || {
            if !crate::workspace_msg::inject_notice(&live, &line) {
                k2_core::log_debug!("[chat/rename] notice not delivered to {}", live.session_id());
            }
        });
    if let Err(e) = spawned {
        k2_core::log_debug!("[chat/rename] could not start the notice: {e}");
    }
}

#[derive(Deserialize)]
struct ReleaseNamesBody {
    provider: String,
    session_id: String,
    #[serde(default)]
    project_path: Option<String>,
}

/// POST chat/release-names (TR6c, "Release old names"): drop a chat's
/// retired names so another chat may take them. Its current name and
/// `ws/<n>` stay. Member floor (route policy), POST-only.
pub fn handle_release_names(is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let parsed: ReleaseNamesBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    let key = parsed.session_id.trim();
    if key.is_empty() || parsed.provider.trim().is_empty() {
        return CliResponse::bad_request("Missing 'provider' or 'session_id' parameter");
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    let project_id = parsed
        .project_path
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .and_then(|p| k2_core::workspace::agent_identity::resolve_project_id(&conn, p));
    match k2_core::workspace_session_handles::release_aliases(&conn, project_id.as_deref(), key) {
        Ok(released) => CliResponse::ok_json(
            serde_json::json!({ "success": true, "released": released }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct TogglePinBody {
    provider: String,
    session_id: String,
    pinned: bool,
}

pub fn handle_toggle_pin(body: &[u8]) -> CliResponse {
    let parsed: TogglePinBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match ch::toggle_pin(&parsed.provider, &parsed.session_id, parsed.pinned) {
        Ok(()) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncChatHistory,
                serde_json::Value::Null,
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
struct MigrateIdeBody {
    project_path: String,
    composer_ids: Vec<String>,
}

pub fn handle_migrate_ide(body: &[u8]) -> CliResponse {
    let parsed: MigrateIdeBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match ch::migrate_ide_sessions(&parsed.project_path, &parsed.composer_ids) {
        Ok(count) => CliResponse::ok_json(
            serde_json::json!({ "migrated": count }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── User archive / restore (Claude physical MOVE; P0) ─────────────────

#[derive(Deserialize)]
struct ArchiveBody {
    project_path: String,
    provider: String,
    session_id: String,
}

/// POST chat/archive — move Claude session into
/// `<project>/.k2/session-archive/user/claude/` (or soft-archive when the
/// live file is already gone). Rejects with 409 if a live PTY still
/// references the session.
pub fn handle_archive(body: &[u8]) -> CliResponse {
    let parsed: ArchiveBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.provider != "claude" {
        return CliResponse::bad_request("only claude archive supported in v1");
    }
    if parsed.session_id.is_empty() || parsed.project_path.is_empty() {
        return CliResponse::bad_request("project_path and session_id required");
    }

    // Live check: refuse to move a transcript a running child still owns.
    for (_name, live) in crate::session_lookup::snapshot_all() {
        if live.is_child_alive()
            && k2_core::workspace::provider_resume::argv_references_session(
                &live.args(),
                &parsed.session_id,
            )
        {
            return CliResponse {
                status: "409 Conflict",
                content_type: "application/json",
                body: serde_json::json!({
                    "error": "session is live; stop the agent before archiving"
                })
                .to_string(),
            };
        }
    }

    // Snapshot title/timestamp from the current list when possible.
    let (title, timestamp) = match ch::list_all_sessions(Some(&parsed.project_path)) {
        Ok(sessions) => {
            let hit = sessions.iter().find(|s| {
                s.session_id == parsed.session_id && s.provider == parsed.provider
            });
            match hit {
                Some(s) => (s.title.clone(), s.timestamp),
                None => {
                    let short = if parsed.session_id.len() >= 8 {
                        &parsed.session_id[..8]
                    } else {
                        &parsed.session_id
                    };
                    (
                        format!("Session {short}…"),
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0),
                    )
                }
            }
        }
        Err(_) => {
            let short = if parsed.session_id.len() >= 8 {
                &parsed.session_id[..8]
            } else {
                &parsed.session_id
            };
            (
                format!("Session {short}…"),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or(0),
            )
        }
    };

    match k2_core::chat_user_archive::archive_user_session(
        &parsed.project_path,
        &parsed.provider,
        &parsed.session_id,
        &title,
        timestamp,
    ) {
        Ok(()) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncChatHistory,
                serde_json::Value::Null,
            );
            CliResponse::ok_json(r#"{"success":true}"#.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST chat/continue-seed — read-only seed for a new chat.
///
/// Body is camelCase (`sessionId`, `projectPath`, `targetProvider`).
/// A non-POST is 405 even though the dispatcher arm is already POST.
pub fn handle_continue_seed(is_post: bool, body: &[u8]) -> CliResponse {
    if !is_post {
        return CliResponse::method_not_allowed();
    }
    let parsed: ContinueSeedBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.provider.trim().is_empty() {
        return CliResponse::bad_request("provider required");
    }
    if parsed.session_id.trim().is_empty() {
        return CliResponse::bad_request("sessionId required");
    }
    if parsed.target_provider.trim().is_empty() {
        return CliResponse::bad_request("targetProvider required");
    }
    let mode = match parsed.mode.as_str() {
        "recent" => k2_core::chat_continue::ContinueMode::Recent,
        "full" => k2_core::chat_continue::ContinueMode::Full,
        _ => return CliResponse::bad_request("mode must be recent or full"),
    };
    match k2_core::chat_continue::build_continue_seed(
        &k2_core::chat_continue::ContinueSeedRequest {
            provider: parsed.provider,
            session_id: parsed.session_id,
            project_path: parsed.project_path,
            mode,
        },
    ) {
        Ok(text) => CliResponse::ok_json(serde_json::json!({ "text": text }).to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContinueSeedBody {
    provider: String,
    session_id: String,
    project_path: String,
    mode: String,
    target_provider: String,
}

/// POST chat/restore — move Claude session back from the user archive.
pub fn handle_restore(body: &[u8]) -> CliResponse {
    let parsed: ArchiveBody = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    if parsed.provider != "claude" {
        return CliResponse::bad_request("only claude archive supported in v1");
    }
    if parsed.session_id.is_empty() || parsed.project_path.is_empty() {
        return CliResponse::bad_request("project_path and session_id required");
    }

    match k2_core::chat_user_archive::restore_user_session(
        &parsed.project_path,
        &parsed.provider,
        &parsed.session_id,
    ) {
        Ok(note) => {
            k2_core::agent_hooks::emit(
                k2_core::agent_hooks::HookEvent::SyncChatHistory,
                serde_json::Value::Null,
            );
            let mut body = serde_json::json!({ "success": true });
            if let Some(note) = note {
                // TR6b: the name was taken while archived.
                body["note"] = serde_json::json!(note);
            }
            CliResponse::ok_json(body.to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k2_core::chat_history::claude_project_hash;
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn error_of(resp: &CliResponse) -> String {
        assert_eq!(resp.status, "400 Bad Request", "{}", resp.body);
        let parsed: serde_json::Value =
            serde_json::from_str(&resp.body).expect("400 body must be JSON");
        assert!(
            parsed.get("text").is_none(),
            "400 must not return a seed: {}",
            resp.body
        );
        parsed
            .get("error")
            .and_then(|v| v.as_str())
            .expect("error string")
            .to_string()
    }

    fn text_of(resp: &CliResponse) -> String {
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let parsed: serde_json::Value =
            serde_json::from_str(&resp.body).expect("200 body must be JSON");
        parsed
            .get("text")
            .and_then(|v| v.as_str())
            .expect("text string")
            .to_string()
    }

    fn post(body: serde_json::Value) -> CliResponse {
        let bytes = serde_json::to_vec(&body).expect("body json");
        handle_continue_seed(true, &bytes)
    }

    fn write_claude(home: &Path, project: &Path, session_id: &str, body: &str) -> PathBuf {
        std::fs::create_dir_all(project).expect("project");
        let hash = claude_project_hash(&project.to_string_lossy());
        let dir = home.join(".claude").join("projects").join(hash);
        std::fs::create_dir_all(&dir).expect("claude dir");
        let path = dir.join(format!("{session_id}.jsonl"));
        std::fs::write(&path, body).expect("transcript");
        path
    }

    #[test]
    fn get_continue_seed_is_405_from_the_get_chain() {
        let params = HashMap::new();
        let via_cli = crate::cli::dispatch("/cli/chat/continue-seed", &params);
        assert_eq!(via_cli.status, "405 Method Not Allowed");
        assert_eq!(via_cli.body, r#"{"error":"POST required"}"#);
        let via_get = crate::misc_routes::dispatch("/cli/chat/continue-seed", &params)
            .expect("GET dispatch must own /cli/chat/continue-seed");
        assert_eq!(via_get.status, "405 Method Not Allowed");
        assert_eq!(via_get.body, r#"{"error":"POST required"}"#);
        assert!(
            !via_cli.body.contains("route not found"),
            "GET must not 404: {}",
            via_cli.body
        );
    }

    #[test]
    fn non_post_handler_is_405() {
        let resp = handle_continue_seed(false, br#"{"provider":"claude"}"#);
        assert_eq!(resp.status, "405 Method Not Allowed");
        assert_eq!(resp.body, r#"{"error":"POST required"}"#);
    }

    #[test]
    fn snake_case_body_is_not_a_seed() {
        let resp = handle_continue_seed(
            true,
            br#"{"provider":"claude","session_id":"abc","project_path":"/tmp/p","mode":"recent","target_provider":"claude"}"#,
        );
        let err = error_of(&resp);
        assert!(
            err.contains("sessionId") || err.contains("missing field"),
            "camelCase fields must be required: {err}"
        );
    }

    #[test]
    fn claude_recent_skips_tool_result() {
        let home = crate::test_support::TempHome::new();
        let project = home.path().join("work");
        let session_id = "sess-route-1";
        let path = write_claude(
            home.path(),
            &project,
            session_id,
            concat!(
                "{\"type\":\"user\",\"message\":{\"content\":\"first ask\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":\"first reply\"}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"TOOL_ONLY_SHOULD_NOT_BE_USER\"}]}}\n",
                "{\"type\":\"user\",\"message\":{\"content\":\"REAL_LAST_USER\"}}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"REAL_LAST_ASSISTANT\"}]}}\n",
            ),
        );
        let before = std::fs::read(&path).expect("before");
        let resp = post(serde_json::json!({
            "provider": "claude",
            "sessionId": session_id,
            "projectPath": project.to_string_lossy(),
            "mode": "recent",
            "targetProvider": "grok",
        }));
        let text = text_of(&resp);
        assert!(text.contains("REAL_LAST_USER"), "{text}");
        assert!(text.contains("REAL_LAST_ASSISTANT"), "{text}");
        assert!(!text.contains("TOOL_ONLY_SHOULD_NOT_BE_USER"), "{text}");
        assert!(!text.contains(path.to_string_lossy().as_ref()), "{text}");
        assert_eq!(std::fs::read(&path).expect("after"), before);
    }

    #[test]
    fn caps_4000_and_48000_round_trip_the_handler() {
        let home = crate::test_support::TempHome::new();
        let project = home.path().join("work");
        let session_id = "sess-route-cap";
        let user = format!("{}TAILMARK", "A".repeat(4_000));
        let assistant = format!("{}ASSISTMARK", "B".repeat(4_000));
        write_claude(
            home.path(),
            &project,
            session_id,
            &format!(
                "{}\n{}\n",
                serde_json::json!({"type":"user","message":{"content": user}}),
                serde_json::json!({"type":"assistant","message":{"content": assistant}}),
            ),
        );
        let recent = text_of(&post(serde_json::json!({
            "provider": "claude",
            "sessionId": session_id,
            "projectPath": project.to_string_lossy(),
            "mode": "recent",
            "targetProvider": "claude",
        })));
        assert!(recent.contains(&"A".repeat(4_000)), "user cap head missing");
        assert!(!recent.contains("TAILMARK"), "{recent}");
        assert!(!recent.contains("ASSISTMARK"), "{recent}");

        let mut body = String::new();
        body.push_str("{\"type\":\"user\",\"message\":{\"content\":\"short title\"}}\n");
        body.push_str("{\"type\":\"assistant\",\"message\":{\"content\":\"done reply\"}}\n");
        body.push_str("PREFIX_OMIT_ME");
        body.push_str(&"x".repeat(60_000));
        body.push_str("SUFFIX_KEEP_ME");
        let path = write_claude(home.path(), &project, session_id, &body);
        let omit = body.chars().count() - 48_000;
        let full = text_of(&post(serde_json::json!({
            "provider": "claude",
            "sessionId": session_id,
            "projectPath": project.to_string_lossy(),
            "mode": "full",
            "targetProvider": "claude",
        })));
        assert!(
            full.starts_with(&format!("Earlier history omitted: {omit} characters.\n")),
            "{full}"
        );
        assert!(full.contains("SUFFIX_KEEP_ME"), "{full}");
        assert!(!full.contains("PREFIX_OMIT_ME"), "{full}");
        assert!(!full.contains(path.to_string_lossy().as_ref()), "{full}");
    }

    #[test]
    fn full_without_transcript_and_no_turns_and_cursor_are_400() {
        let home = crate::test_support::TempHome::new();
        let project = home.path().join("work");
        std::fs::create_dir_all(&project).expect("project");
        let missing = post(serde_json::json!({
            "provider": "claude",
            "sessionId": "missing-sess",
            "projectPath": project.to_string_lossy(),
            "mode": "full",
            "targetProvider": "claude",
        }));
        assert_eq!(error_of(&missing), "no readable transcript");

        write_claude(
            home.path(),
            &project,
            "tool-only",
            "{\"type\":\"user\",\"message\":{\"content\":[{\"type\":\"tool_result\",\"content\":\"ONLY_TOOL\"}]}}\n",
        );
        let no_turns = post(serde_json::json!({
            "provider": "claude",
            "sessionId": "tool-only",
            "projectPath": project.to_string_lossy(),
            "mode": "recent",
            "targetProvider": "claude",
        }));
        assert_eq!(error_of(&no_turns), "no turns");

        let project_str = project.to_string_lossy();
        let root = k2_core::chat_history::resolve_root_project_path(&project_str);
        let hash = k2_core::chat_history::md5_hex(root.as_bytes());
        let dir = home
            .path()
            .join(".cursor")
            .join("chats")
            .join(hash)
            .join("cursor-sess");
        std::fs::create_dir_all(&dir).expect("cursor dir");
        std::fs::write(dir.join("store.db"), "CURSOR_DB_BYTES_MUST_NOT_LEAK").expect("db");
        let cursor = post(serde_json::json!({
            "provider": "cursor",
            "sessionId": "cursor-sess",
            "projectPath": project.to_string_lossy(),
            "mode": "full",
            "targetProvider": "claude",
        }));
        let err = error_of(&cursor);
        assert_eq!(err, "no readable transcript");
        assert!(!cursor.body.contains("CURSOR_DB_BYTES_MUST_NOT_LEAK"));
    }

    // ── Thread survives a tab rename (S3/TR8/TR6c) ─────────────────────

    /// A workspace with handle `<h>` and one unnamed Claude sidecar at
    /// ordinal 1. Returns (project_id, path, handle, conversation id).
    fn seed_rename_sidecar(label: &str) -> (String, String, String, String) {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let id = uuid::Uuid::new_v4().to_string();
        let handle = format!("{label}{}", &id[..8]);
        let path = format!("/tmp/chat-rename-{handle}");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            rusqlite::params![id, handle, path],
        )
        .expect("seed project");
        let conv = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspace_tab_sessions \
             (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
            rusqlite::params![id, format!("pane-{conv}"), format!("tab-pane-{conv}"), conv],
        )
        .expect("tab");
        k2_core::workspace_session_handles::allocate_ordinal(&conn, &id, &conv).expect("ordinal");
        (id, path, handle, conv)
    }

    fn rename_body(conv: &str, name: &str, path: &str) -> Vec<u8> {
        serde_json::json!({
            "provider": "claude",
            "session_id": conv,
            "custom_name": name,
            "project_path": path,
        })
        .to_string()
        .into_bytes()
    }

    /// Every `session_address_changed` for `conv` on the bus right now.
    fn address_events_for(
        rx: &mut tokio::sync::broadcast::Receiver<crate::session_events::SessionEvent>,
        conv: &str,
    ) -> Vec<(Option<String>, String, String, String)> {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(crate::session_events::SessionEvent::SessionAddressChanged {
                    workspace_path,
                    pane_group_id,
                    conversation_id,
                    address,
                    previous_address,
                }) if conversation_id == conv => {
                    out.push((pane_group_id, workspace_path, address, previous_address));
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        out
    }

    /// Test 12: a rename emits exactly one `session_address_changed`
    /// carrying old and new; a blank rename emits one back to the ordinal;
    /// renaming to the same name again emits nothing. The response names
    /// both addresses.
    #[test]
    fn rename_emits_one_address_event_per_change() {
        let (_pid, path, handle, conv) = seed_rename_sidecar("chrnev");
        let mut rx = crate::session_events::subscribe();

        let resp = handle_rename(&rename_body(&conv, "Reviewer", &path));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(body["address"], format!("{handle}/reviewer"));
        assert_eq!(body["previousAddress"], format!("{handle}/1"));
        let events = address_events_for(&mut rx, &conv);
        assert_eq!(
            events,
            vec![(
                Some(format!("pane-{conv}")),
                path.clone(),
                format!("{handle}/reviewer"),
                format!("{handle}/1"),
            )],
            "exactly one event with old and new"
        );

        let same = handle_rename(&rename_body(&conv, "Reviewer", &path));
        assert_eq!(same.status, "200 OK", "{}", same.body);
        assert!(address_events_for(&mut rx, &conv).is_empty(), "no change, no event");

        let cleared = handle_rename(&rename_body(&conv, "   ", &path));
        assert_eq!(cleared.status, "200 OK", "{}", cleared.body);
        let events = address_events_for(&mut rx, &conv);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].2, format!("{handle}/1"), "blank goes back to the ordinal");
        assert_eq!(events[0].3, format!("{handle}/reviewer"));
    }

    /// TR8: the notice is one fixed line, and a dormant chat (no live
    /// session) gets nothing — a rename never wakes.
    #[test]
    fn rename_notice_is_one_line_and_never_reaches_a_dormant_chat() {
        assert_eq!(
            rename_notice_line("k2/3", "k2/reviewer"),
            "[k2] This chat's address is now k2/reviewer (was k2/3). Old addresses still reach you."
        );
        let (_pid, path, _handle, conv) = seed_rename_sidecar("chrnnt");
        let resp = handle_rename(&rename_body(&conv, "Reviewer", &path));
        assert_eq!(resp.status, "200 OK", "{}", resp.body);
        assert!(
            recorded_notices().iter().all(|(k, _)| k != &conv),
            "dormant chats get no notice: {:?}",
            recorded_notices()
        );
    }

    /// TR6c: Release old names frees a chat's retired names (POST only),
    /// so another chat may take them.
    #[test]
    fn release_names_frees_retired_names() {
        let (pid, path, handle, conv) = seed_rename_sidecar("chrnrl");
        let other = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO workspace_tab_sessions \
                 (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
                 VALUES (?1, ?2, ?3, ?4, 'claude', unixepoch())",
                rusqlite::params![pid, format!("pane-{other}"), format!("tab-pane-{other}"), other],
            )
            .expect("tab");
        }
        assert_eq!(handle_rename(&rename_body(&conv, "Reviewer", &path)).status, "200 OK");
        assert_eq!(handle_rename(&rename_body(&conv, "Critic", &path)).status, "200 OK");
        let refused = handle_rename(&rename_body(&other, "Reviewer", &path));
        assert_eq!(refused.status, "400 Bad Request", "{}", refused.body);
        assert!(refused.body.contains(&format!("{handle}/1 (now {handle}/critic)")), "{}", refused.body);

        let get = handle_release_names(false, b"");
        assert_eq!(get.status, "405 Method Not Allowed");
        let via_get = crate::misc_routes::dispatch("/cli/chat/release-names", &HashMap::new())
            .expect("GET dispatch owns /cli/chat/release-names");
        assert_eq!(via_get.status, "405 Method Not Allowed");

        let released = handle_release_names(
            true,
            serde_json::json!({ "provider": "claude", "session_id": conv, "project_path": path })
                .to_string()
                .as_bytes(),
        );
        assert_eq!(released.status, "200 OK", "{}", released.body);
        let body: serde_json::Value = serde_json::from_str(&released.body).expect("json");
        assert_eq!(body["released"], serde_json::json!(["reviewer"]));
        assert_eq!(handle_rename(&rename_body(&other, "Reviewer", &path)).status, "200 OK");
    }
}
