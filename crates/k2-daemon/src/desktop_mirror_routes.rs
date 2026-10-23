//! Desktop mirrors (0.46.4): the `/cli/*` routes the desktop app's Tauri
//! commands forward to now that the app no longer links k2-core.
//!
//! Each one replaces a Tauri command that only this computer's own app
//! could call, and the renderer marks them this-computer-only
//! (`src/renderer/lib/local-only-actions.ts`). So they stay this
//! computer's: the policy rows are `NoLogin` (every Connect login is
//! turned away at `role_gate`), and [`handle`] answers anything but the
//! LOCAL owner token (an agent passport, an App pass, an API key) with
//! 403 `desktop_local_only`. Same shape as the Home avatar cache
//! (`home_avatar_routes`). All seven are POST-only (405 on GET) with a
//! capped JSON body.
//!
//! `GET /cli/inbox/count` is NOT here: it is the root-only count of what
//! `GET /cli/inbox/list` already returns in full, so it keeps that route's
//! floor (Member) and its scoped-inbox gate.

use crate::cli_response::CliResponse;

/// Every desktop mirror route (all POST).
pub const ROUTES: &[&str] = &[
    "/cli/agents/build-launch",
    "/cli/agents/editor-context",
    "/cli/agents/teardown-workspace",
    "/cli/heartbeat/preview-schedule",
    "/cli/heartbeat/show-sessions",
    "/cli/skills/list",
    "/cli/workspace/session-get",
];

/// Body cap: the largest body is build-launch's few optional strings.
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

fn resp(status: &'static str, body: serde_json::Value) -> CliResponse {
    CliResponse { status, content_type: "application/json", body: body.to_string() }
}

pub fn local_only() -> CliResponse {
    resp(
        "403 Forbidden",
        serde_json::json!({
            "error": "desktop_local_only",
            "message": "This route belongs to this computer's own K2 app. Only this computer's owner token may call it.",
        }),
    )
}

pub fn too_large() -> CliResponse {
    resp(
        "413 Payload Too Large",
        serde_json::json!({ "error": "body_too_large", "max_bytes": MAX_BODY }),
    )
}

/// Run one desktop mirror. `owner` = the request carried the LOCAL owner
/// token (`token_is_owner`); anything else is refused before any work.
pub fn handle(path: &str, owner: bool, body: &[u8]) -> CliResponse {
    if !owner {
        return local_only();
    }
    match path {
        "/cli/agents/build-launch" => crate::agents_routes::handle_build_launch(body),
        "/cli/agents/editor-context" => crate::agents_routes::handle_editor_context(body),
        "/cli/agents/teardown-workspace" => crate::agents_routes::handle_teardown_workspace(body),
        "/cli/heartbeat/preview-schedule" => crate::heartbeat_routes::handle_preview_schedule(body),
        "/cli/heartbeat/show-sessions" => crate::heartbeat_routes::handle_show_heartbeat_sessions(body),
        "/cli/skills/list" => crate::skills_routes::handle_list(body),
        "/cli/workspace/session-get" => crate::agents_routes::handle_workspace_session_get(body),
        _ => CliResponse::not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A remote Connect login never holds the owner token and an agent
    /// passport is `<sid>.<secret>`; both reach `handle` as `owner = false`
    /// and must be refused on EVERY mirror before any k2-core call (a
    /// teardown body naming a real path must not touch it).
    #[test]
    fn every_mirror_refuses_a_non_owner_caller() {
        let dir = std::env::temp_dir().join(format!("k2-mirror-refuse-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".k2")).expect("mkdir");
        let marker = dir.join(".k2").join("keep.txt");
        std::fs::write(&marker, "x").expect("write marker");
        let body = serde_json::json!({
            "project_path": dir.to_string_lossy(),
            "project_id": "p1",
            "agent_name": "a",
            "mode": "full",
            "schedule_json": "{}",
            "count": 1,
        })
        .to_string();
        for p in ROUTES {
            let r = handle(p, false, body.as_bytes());
            assert_eq!(r.status, "403 Forbidden", "{p}: {}", r.body);
            assert!(r.body.contains("desktop_local_only"), "{p}: {}", r.body);
        }
        assert!(marker.exists(), "a refused teardown must not touch the workspace");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn owner_reaches_the_handlers() {
        // The owner token gets the handler's own answer (here: its 400 for
        // an empty body), never the local-only refusal.
        for p in ROUTES {
            if *p == "/cli/heartbeat/preview-schedule" {
                continue; // an empty body is a valid (empty) preview
            }
            let r = handle(p, true, b"{}");
            assert_eq!(r.status, "400 Bad Request", "{p}: {}", r.body);
            assert!(!r.body.contains("desktop_local_only"), "{p}");
        }
        let r = handle("/cli/heartbeat/preview-schedule", true, b"{}");
        assert_eq!(r.status, "200 OK", "{}", r.body);
        assert_eq!(r.body, "[]");
    }

    #[test]
    fn the_gate_inputs_are_owner_only() {
        let owner = "mirror-owner-token";
        assert!(crate::routes::http::token_is_owner(&format!("token={owner}"), owner));
        // An agent passport (`<sid>.<secret>`) and any other token are not owner.
        assert!(!crate::routes::http::token_is_owner("token=0b5e3f1c-1111-2222-3333-444455556666.secret", owner));
        assert!(!crate::routes::http::token_is_owner("token=k2skn_app_pass", owner));
        assert!(!crate::routes::http::token_is_owner("", owner));
    }
}
