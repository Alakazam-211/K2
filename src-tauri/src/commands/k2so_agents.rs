//! K2SO Agent system — the desktop's agent / heartbeat / workspace-session
//! commands.
//!
//! 0.46.4: this app no longer links k2-core. Every command here that used
//! to run a k2-core fn in this process (opening ~/.k2/k2.db, writing
//! workspace files) now forwards to the daemon route that runs the SAME
//! fn with the same arguments (`DaemonClient::mirror_*`), and returns its
//! JSON value unchanged. A k2-core `Err(e)` arrives as the bare `e`, as
//! before. Commands the renderer never invoked were removed rather than
//! forwarded (see the build-speed inventory). Names and argument shapes
//! are unchanged, so every `invoke(...)` call site keeps working.
//!
//! Every daemon-calling command is `async` and runs its blocking HTTP call
//! on the blocking pool ([`off_main`]): a sync Tauri command runs on the
//! main thread, so a slow daemon (10 s client timeout) would freeze the UI.

use serde_json::{json, Value};
use tauri::Emitter;

use crate::daemon_client::{off_main, DaemonClient};

fn daemon() -> Result<DaemonClient, String> {
    DaemonClient::try_connect()
}

/// `workspace::agent::list` → `GET /cli/agents/list`.
#[tauri::command]
pub async fn k2so_agents_list(project_path: String) -> Result<Value, String> {
    off_main(move || daemon()?.mirror_get("/cli/agents/list", &[("project", &project_path)])).await
}

/// Full-fat wake-launch builder (UI "Launch" button + Chat tab):
/// `workspace::agent_launch::k2so_agents_build_launch` →
/// `POST /cli/agents/build-launch`. `heartbeat_name` is the
/// heartbeat-scoped resume target (React callers leave it `None`).
#[tauri::command]
pub async fn k2so_agents_build_launch(
    project_path: String,
    agent_name: String,
    agent_cli_command: Option<String>,
    wakeup_override: Option<String>,
    skip_fork_session: Option<bool>,
    heartbeat_name: Option<String>,
) -> Result<Value, String> {
    off_main(move || {
        daemon()?.mirror_post(
            "/cli/agents/build-launch",
            &json!({
                "project_path": project_path,
                "agent_name": agent_name,
                "agent_cli_command": agent_cli_command,
                "wakeup_override": wakeup_override,
                "skip_fork_session": skip_fork_session,
                "heartbeat_name": heartbeat_name,
            }),
        )
    })
    .await
}

/// Per-harness canonical state, read-only:
/// `workspace::canonical::detect_canonical_state` →
/// `POST /cli/canonical/detect-state`.
#[tauri::command]
pub async fn k2so_detect_canonical_state(project_path: String) -> Result<Value, String> {
    off_main(move || {
        daemon()?.mirror_post("/cli/canonical/detect-state", &json!({ "project_path": project_path }))
    })
    .await
}

/// Review queue (agents with finished work in worktree branches):
/// `workspace::reviews::review_queue` → `GET /cli/reviews`.
#[tauri::command]
pub async fn k2so_agents_review_queue(project_path: String) -> Result<Value, String> {
    off_main(move || daemon()?.mirror_get("/cli/reviews", &[("project", &project_path)])).await
}

/// `workspace::session::k2so_agents_lock` → `POST /cli/agents/lock`.
#[tauri::command]
pub async fn k2so_agents_lock(
    project_path: String,
    agent_name: String,
    terminal_id: Option<String>,
    owner: Option<String>,
) -> Result<(), String> {
    off_main(move || {
        let mut params: Vec<(&str, &str)> = vec![("project", &project_path), ("agent", &agent_name)];
        if let Some(t) = terminal_id.as_deref() {
            params.push(("terminal_id", t));
        }
        if let Some(o) = owner.as_deref() {
            params.push(("owner", o));
        }
        daemon()?.cli_post_query("/cli/agents/lock", &params).map(|_| ())
    })
    .await
}

/// 0.38.0 — broadcast across this app's windows when the pinned-chat
/// refresh button is clicked. The same `chat:refreshed` event (payload
/// `{projectPath}`) k2-core's `k2so_chat_refresh_broadcast` emitted
/// through the in-process hook sink; emitted directly now (no daemon call,
/// so it stays sync).
#[tauri::command]
pub fn k2so_chat_refresh_broadcast(app: tauri::AppHandle, project_path: String) -> Result<(), String> {
    let _ = app.emit(k2_client::events::CHAT_REFRESHED, json!({ "projectPath": project_path }));
    Ok(())
}

/// Next `count` fire times for a schedule (Settings preview):
/// `scheduler::preview_fire_times` → `POST /cli/heartbeat/preview-schedule`.
#[tauri::command]
pub async fn k2so_agents_preview_schedule(
    mode: String,
    schedule_json: String,
    count: u32,
) -> Result<Value, String> {
    off_main(move || {
        daemon()?.mirror_post(
            "/cli/heartbeat/preview-schedule",
            &json!({ "mode": mode, "schedule_json": schedule_json, "count": count }),
        )
    })
    .await
}

/// `heartbeats::k2so_heartbeat_list` → `GET /cli/heartbeat/list`.
#[tauri::command]
pub async fn k2so_heartbeat_list(project_path: String) -> Result<Value, String> {
    off_main(move || daemon()?.mirror_get("/cli/heartbeat/list", &[("project", &project_path)])).await
}

/// 0.38.3 — most recent heartbeat fire records across ALL projects (the
/// Heartbeats settings page audit log). Default limit 100 fires.
/// `heartbeats::k2so_heartbeat_fires_list_all` → `GET /cli/heartbeat/fires-list-all`.
#[tauri::command]
pub async fn k2so_heartbeat_fires_list_all(limit: Option<i64>) -> Result<Value, String> {
    off_main(move || {
        let limit_s = limit.map(|l| l.to_string());
        let mut params: Vec<(&str, &str)> = Vec::new();
        if let Some(l) = limit_s.as_deref() {
            params.push(("limit", l));
        }
        daemon()?.mirror_get("/cli/heartbeat/fires-list-all", &params)
    })
    .await
}

/// 0.38.3 — every active heartbeat across ALL workspaces with the parent
/// project's name + path. `heartbeats::k2so_heartbeat_list_all` →
/// `GET /cli/heartbeat/list-all`.
#[tauri::command]
pub async fn k2so_heartbeat_list_all() -> Result<Value, String> {
    off_main(|| daemon()?.mirror_get("/cli/heartbeat/list-all", &[])).await
}

/// 1 = each scheduled heartbeat fire opens a background tab.
/// `heartbeats::k2so_workspace_get_show_heartbeat_sessions` →
/// `POST /cli/heartbeat/show-sessions`.
#[tauri::command]
pub async fn k2so_workspace_get_show_heartbeat_sessions(project_path: String) -> Result<Value, String> {
    off_main(move || {
        daemon()?.mirror_post("/cli/heartbeat/show-sessions", &json!({ "project_path": project_path }))
    })
    .await
}

/// Heartbeat delivery drop-down — set where this heartbeat's wakeup
/// goes. Thin facade over the daemon HTTP route
/// `/cli/heartbeat/set-session` (daemon owns the write), mirroring
/// `workspace_session_set_session_id`. Supersedes the
/// `set_use_workspace_session` checkbox toggle on the Settings tiles.
///
/// `mode` is `pinned` (the workspace's pinned chat), `auto` (own
/// session, new on next fire), or `session` (an explicit saved
/// session — `session_id` + `provider` ride along, mode=session only).
#[tauri::command]
pub async fn k2so_heartbeat_set_session(
    project_path: String,
    name: String,
    mode: String,
    session_id: Option<String>,
    provider: Option<String>,
) -> Result<String, String> {
    off_main(move || {
        let client = daemon()?;
        let mut params: Vec<(&str, &str)> = vec![
            ("project", &project_path),
            ("name", &name),
            ("mode", &mode),
        ];
        if let Some(sid) = session_id.as_deref() {
            params.push(("session_id", sid));
        }
        if let Some(prov) = provider.as_deref() {
            params.push(("provider", prov));
        }
        client.cli_post_query("/cli/heartbeat/set-session", &params)
    })
    .await
}

// ── Heartbeat Scheduler — Phase 2 Unit 7c ───────────────────────────────
//
// Thin daemon-HTTP proxies (`/cli/heartbeat/{install-launchd,
// uninstall-launchd, apply-wake-scheduler}`). The wake-scheduler settings
// they forward are read from the daemon-owned `~/.k2/settings.json`
// through k2-client's read-only view (same fields + defaults as
// `k2_core::app_settings::load`).

/// Install the heartbeat scheduler via the daemon.
#[tauri::command]
pub async fn k2so_agents_install_heartbeat() -> Result<(), String> {
    off_main(|| {
        let client = daemon()?;
        let cfg = k2_client::app_settings_view::load().wake_scheduler;
        let interval_secs = cfg.interval_minutes.max(1) * 60;
        let body = json!({
            "interval_seconds": interval_secs,
            "wake_system": cfg.wake_system,
        });
        let _ = client.cli_post_json("/cli/heartbeat/install-launchd", &body)?;
        Ok(())
    })
    .await
}

/// The apply-wake-scheduler round trip (blocking; callers run it off the
/// main thread).
fn apply_wake_scheduler_blocking() -> Result<String, String> {
    let client = daemon()?;
    let cfg = k2_client::app_settings_view::load().wake_scheduler;
    let body = json!({
        "mode": cfg.mode,
        "interval_minutes": cfg.interval_minutes,
        "wake_system": cfg.wake_system,
    });
    let resp = client.cli_post_json("/cli/heartbeat/apply-wake-scheduler", &body)?;
    let v: Value = serde_json::from_str(&resp)
        .map_err(|e| format!("decode apply-wake-scheduler: {e}"))?;
    Ok(v.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string())
}

/// Apply the user's Wake Scheduler settings — daemon decides between
/// install / uninstall based on `mode`.
#[tauri::command]
pub async fn k2so_agents_apply_wake_scheduler() -> Result<String, String> {
    off_main(apply_wake_scheduler_blocking).await
}

/// Uninstall the heartbeat scheduler via the daemon.
#[tauri::command]
pub async fn k2so_agents_uninstall_heartbeat() -> Result<(), String> {
    off_main(|| {
        let client = daemon()?;
        let _ = client.cli_post_json("/cli/heartbeat/uninstall-launchd", &json!({}))?;
        Ok(())
    })
    .await
}

/// Refresh the heartbeat-scheduler lifecycle when the toggle changes —
/// the same idempotent re-apply as `k2so_agents_apply_wake_scheduler`
/// (its result is ignored, as before).
#[tauri::command]
pub async fn k2so_agents_update_heartbeat_projects() -> Result<(), String> {
    off_main(|| {
        let _ = apply_wake_scheduler_blocking();
        Ok(())
    })
    .await
}

/// AIFileEditor agent-editing context:
/// `workspace::agent_editor::k2so_agents_get_editor_context` →
/// `POST /cli/agents/editor-context`.
#[tauri::command]
pub async fn k2so_agents_get_editor_context(project_path: String, agent_name: String) -> Result<Value, String> {
    off_main(move || {
        daemon()?.mirror_post(
            "/cli/agents/editor-context",
            &json!({ "project_path": project_path, "agent_name": agent_name }),
        )
    })
    .await
}

/// The workspace's `WorkspaceSession` row or `null`:
/// `workspace::relations::workspace_session_get` →
/// `POST /cli/workspace/session-get`.
#[tauri::command]
pub async fn workspace_session_get(project_id: String) -> Result<Value, String> {
    off_main(move || daemon()?.mirror_post("/cli/workspace/session-get", &json!({ "project_id": project_id })))
        .await
}

/// 0.37.12 — explicitly set the pinned chat tab's Claude session id for a
/// workspace. Thin facade over `/cli/workspace/set-chat-session`.
#[tauri::command]
pub async fn workspace_session_set_session_id(
    project_path: String,
    session_id: String,
) -> Result<String, String> {
    off_main(move || {
        daemon()?.cli_post_query(
            "/cli/workspace/set-chat-session",
            &[("project", &project_path), ("session_id", &session_id)],
        )
    })
    .await
}

/// `workspace::teardown::k2so_agents_teardown_workspace` →
/// `POST /cli/agents/teardown-workspace`.
#[tauri::command]
pub async fn k2so_agents_teardown_workspace(project_path: String, mode: String) -> Result<Value, String> {
    off_main(move || {
        daemon()?.mirror_post(
            "/cli/agents/teardown-workspace",
            &json!({ "project_path": project_path, "mode": mode }),
        )
    })
    .await
}

// ══════════════════════════════════════════════════════════════════════
// Migration-safety tests (Phase 7c/7d invariants)
// ══════════════════════════════════════════════════════════════════════
//
// These tests pin down the "never lose user data" contract. Every
// migration path we ship must:
//   1. Archive user-authored content before mutating or deleting it.
//   2. Be idempotent (running twice never doubles or re-loses content).
//   3. Return preserved content from strip_workspace_skill_tail so
//      append_workspace_source_regions can re-emit it losslessly.
//   4. Never stack duplicate USER_NOTES sentinels / placeholder comments.

#[cfg(test)]
mod pure_helper_tests {
    //! Tests for the I/O-free helpers extracted from Tauri command
    //! handlers in Phase C of the testability work. Each helper is a
    //! pure function (no fs, no db, no Tauri state) so these tests
    //! run in microseconds and cover edge cases that would otherwise
    //! require scaffolding a full workspace.
    // k2-core is a dev-dependency only: these test k2-core helpers.
    use k2_core::workspace::work_item::WorkItem;
    // Resolved directly from core: the module-level pub-use re-exports
    // these once rode were removed in the 0.40.31 warning-zero pass.
    use k2_core::workspace::agent::update_agent_md_field;
    use k2_core::workspace::wake_prompts::{
        compose_agent_wake_from_body, compose_manager_wake_from_body,
        WAKEUP_TEMPLATE_WORKSPACE,
    };
    use k2_core::workspace::work_item::parse_work_item_content;

    // ── update_agent_md_field ────────────────────────────────────
    #[test]
    fn update_field_replaces_frontmatter_value() {
        let content = "---\nrole: old role\ntype: custom\n---\n# Agent body\n";
        let updated = update_agent_md_field(content, "role", "new role").unwrap();
        assert!(updated.contains("role: new role"), "got: {}", updated);
        assert!(!updated.contains("role: old role"));
        assert!(updated.contains("type: custom"), "other keys preserved: {}", updated);
        assert!(updated.contains("# Agent body"), "body preserved: {}", updated);
    }

    #[test]
    fn update_field_replaces_section_body() {
        let content = "---\nrole: x\n---\n# Agent\n\n## Work Sources\n\nold content\n\n## Other\n\nkeep\n";
        let updated = update_agent_md_field(content, "Work Sources", "new content").unwrap();
        assert!(updated.contains("## Work Sources\n\nnew content"), "got: {}", updated);
        assert!(!updated.contains("old content"));
        assert!(updated.contains("## Other\n\nkeep"), "trailing section preserved: {}", updated);
    }

    #[test]
    fn update_field_appends_missing_section() {
        let content = "---\nrole: x\n---\n# Agent\n\n## Existing\n\ntext\n";
        let updated = update_agent_md_field(content, "New Section", "added body").unwrap();
        assert!(updated.contains("## New Section\n\nadded body"), "got: {}", updated);
        assert!(updated.contains("## Existing"), "existing preserved");
    }

    #[test]
    fn update_field_replaces_last_section_to_end_of_body() {
        // Edge case: section has no following `## ` so end-of-body
        // is the boundary. Verifies the .unwrap_or(body.len()) path.
        let content = "---\nrole: x\n---\n# Agent\n\n## Tail\n\nold tail content\n";
        let updated = update_agent_md_field(content, "Tail", "new tail").unwrap();
        assert!(updated.contains("## Tail\n\nnew tail"));
        assert!(!updated.contains("old tail content"));
    }

    #[test]
    fn update_field_rejects_missing_frontmatter() {
        let content = "# Just body\n\nno frontmatter here\n";
        let err = update_agent_md_field(content, "role", "x").unwrap_err();
        assert!(err.contains("missing frontmatter"), "got: {}", err);
    }

    #[test]
    fn update_field_rejects_unterminated_frontmatter() {
        let content = "---\nrole: x\nnever-closed\n# body\n";
        let err = update_agent_md_field(content, "role", "y").unwrap_err();
        assert!(err.contains("Invalid frontmatter"), "got: {}", err);
    }

    #[test]
    fn update_field_frontmatter_update_preserves_body_exactly() {
        // The body section after --- must be byte-identical when only
        // a frontmatter key is updated. Regression guard for the
        // extraction: the pre-refactor code stitched body back in
        // verbatim, and we must preserve that.
        let body = "\n# Heading\n\nLine one.\nLine two.\n\n## Sub\n\nMore.\n";
        let content = format!("---\nrole: a\n---{}", body);
        let updated = update_agent_md_field(&content, "role", "b").unwrap();
        assert!(updated.ends_with(body), "body not byte-preserved: {}", updated);
    }

    #[test]
    fn update_field_handles_value_containing_colon() {
        // Values with colons (URLs, ratio notation) must survive the
        // split_once logic and round-trip correctly.
        let content = "---\nrole: old\n---\n";
        let updated = update_agent_md_field(content, "role", "URL: https://example.com/path").unwrap();
        assert!(updated.contains("role: URL: https://example.com/path"), "got: {}", updated);
    }

    // ── compose_manager_wake_from_body ───────────────────────────
    //
    // P8 retired the "K2SO Heartbeat Wake — Workspace Manager" boilerplate
    // preamble that used to wrap the body. The composer now returns the
    // wakeup body verbatim (frontmatter stripped). These tests exercise
    // the post-P8 contract: body content survives, fallback template
    // kicks in for empty/missing input, no boilerplate added.

    #[test]
    fn compose_manager_wake_uses_provided_body() {
        let out = compose_manager_wake_from_body(Some("custom manager instructions"));
        assert!(out.contains("custom manager instructions"), "body inlined");
        // No K2SO boilerplate prefix anymore — body is the message.
        assert!(!out.contains("K2SO Heartbeat Wake"), "preamble retired in P8: {}", out);
    }

    #[test]
    fn compose_manager_wake_falls_back_when_body_none() {
        let out = compose_manager_wake_from_body(None);
        // Fallback uses WAKEUP_TEMPLATE_WORKSPACE — assert its trim()'d
        // first line is in the output.
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(!template_lead.is_empty());
        assert!(
            out.contains(template_lead),
            "expected template fallback to contain first line '{}', got: {}",
            template_lead,
            out
        );
    }

    #[test]
    fn compose_manager_wake_falls_back_when_body_is_empty_string() {
        // A disk read returning "" after frontmatter strip must hit
        // the fallback — not silently emit an empty wake prompt.
        let out = compose_manager_wake_from_body(Some(""));
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(out.contains(template_lead), "expected template fallback, got: {}", out);
    }

    #[test]
    fn compose_manager_wake_strips_frontmatter_from_body() {
        // If the disk body has its own frontmatter (e.g. a scaffolded
        // WAKEUP.md with metadata), strip_frontmatter must run before
        // the empty-check.
        let body = "---\ntitle: foo\n---\nActual wake instructions here.";
        let out = compose_manager_wake_from_body(Some(body));
        assert!(!out.contains("title: foo"), "frontmatter leaked: {}", out);
        assert!(out.contains("Actual wake instructions here"), "body survived: {}", out);
    }

    // ── compose_agent_wake_from_body ─────────────────────────────
    #[test]
    fn compose_agent_wake_returns_none_on_none_input() {
        assert!(compose_agent_wake_from_body(None).is_none());
    }

    #[test]
    fn compose_agent_wake_returns_body_verbatim() {
        // P8: composer returns the body itself (frontmatter stripped),
        // no boilerplate preamble. The wakeup.md content is the message.
        let out = compose_agent_wake_from_body(Some("agent instructions"))
            .expect("body present -> Some");
        assert!(out.contains("agent instructions"), "body in output: {}", out);
        // No "K2SO Heartbeat Wake" preamble anymore.
        assert!(!out.contains("K2SO Heartbeat Wake"), "preamble retired in P8: {}", out);
    }

    #[test]
    fn compose_agent_wake_strips_frontmatter() {
        // P8: composer now strips frontmatter symmetrically with the
        // manager composer. Pre-P8 it left frontmatter intact and
        // expected callers to strip; post-P8 the composer owns it.
        let body = "---\nname: foo\n---\nbody";
        let out = compose_agent_wake_from_body(Some(body)).unwrap();
        assert!(!out.contains("name: foo"), "frontmatter stripped: {}", out);
        assert!(out.contains("body"), "body survived: {}", out);
    }

    #[test]
    fn compose_agent_wake_returns_none_for_empty_body() {
        // P8: empty body (after frontmatter strip) returns None so
        // smart_launch can record a "wakeup body empty" audit instead
        // of firing claude with no prompt.
        let body = "---\ndescription:\n---\n\n";
        assert!(compose_agent_wake_from_body(Some(body)).is_none());
    }

    // ── parse_work_item_content ──────────────────────────────────
    #[test]
    fn parse_work_item_full_frontmatter() {
        let content = "---\ntitle: Add OAuth\npriority: high\ntype: feature\nsource: feedback\ncreated: 2026-04-01\nassigned_by: user\n---\n\nBody text here that describes the work.";
        let item = parse_work_item_content(content, "add-oauth.md", "inbox");
        assert_eq!(item.title, "Add OAuth");
        assert_eq!(item.priority, "high");
        assert_eq!(item.item_type, "feature");
        assert_eq!(item.source, "feedback");
        assert_eq!(item.assigned_by, "user");
        assert_eq!(item.folder, "inbox");
        assert_eq!(item.filename, "add-oauth.md");
        assert!(item.body_preview.contains("Body text here"));
    }

    #[test]
    fn parse_work_item_missing_fields_use_defaults() {
        let content = "---\ntitle: minimal\n---\nbody";
        let item = parse_work_item_content(content, "m.md", "active");
        assert_eq!(item.title, "minimal");
        assert_eq!(item.priority, "normal"); // default
        assert_eq!(item.item_type, "task"); // default
        assert_eq!(item.source, "manual"); // default
        assert_eq!(item.assigned_by, "unknown"); // default
    }

    #[test]
    fn parse_work_item_no_frontmatter_defaults_all_but_body() {
        let content = "just a body with no metadata";
        let item = parse_work_item_content(content, "raw.md", "inbox");
        assert_eq!(item.title, "");
        assert_eq!(item.body_preview, "just a body with no metadata");
    }

    #[test]
    fn parse_work_item_body_preview_truncates_over_120_chars() {
        let long_body = "x".repeat(300);
        let content = format!("---\ntitle: t\n---\n{}", long_body);
        let item = parse_work_item_content(&content, "l.md", "inbox");
        // Preview is 120 + "..." — exact char count matters.
        assert!(item.body_preview.ends_with("..."), "preview: {:?}", item.body_preview);
        let without_ellipsis = item.body_preview.trim_end_matches("...");
        assert_eq!(without_ellipsis.chars().count(), 120);
    }

    // ── FakeFs-driven demonstration ──────────────────────────────
    //
    // These tests show the end-state pattern: use FakeFs to scaffold
    // a workspace tree, call the Fs trait to read content, then feed
    // that content into the pure parser. No tempdir, no disk I/O.
    //
    // Once `read_work_item` is threaded with `&dyn Fs`, these tests
    // can drop the manual read_to_string and just pass the fs into a
    // higher-level helper. For now they demonstrate the pattern and
    // prove the integration (pure parser + FakeFs storage).

    #[test]
    fn fake_fs_scaffolds_agent_work_tree_and_parses_items() {
        use k2_core::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        fs.insert_tree(
            Path::new("/proj/.k2so/agents/backend-eng/work"),
            serde_json::json!({
                "inbox": {
                    "build-oauth.md": "---\ntitle: Build OAuth\npriority: high\ntype: feature\n---\n\nOAuth endpoints required.",
                    "fix-crash.md": "---\ntitle: Fix startup crash\npriority: urgent\ntype: bug\nsource: crash\n---\n\nCrashes on launch.",
                },
                "active": {},
                "done": {},
            }),
        );

        let inbox_dir = Path::new("/proj/.k2so/agents/backend-eng/work/inbox");
        let mut entries = fs.read_dir(inbox_dir).unwrap();
        entries.sort();

        let items: Vec<WorkItem> = entries
            .iter()
            .map(|p| {
                let content = fs.read_to_string(p).unwrap();
                let filename = p.file_name().unwrap().to_string_lossy();
                parse_work_item_content(&content, &filename, "inbox")
            })
            .collect();

        assert_eq!(items.len(), 2);
        let oauth = items.iter().find(|i| i.filename == "build-oauth.md").unwrap();
        assert_eq!(oauth.title, "Build OAuth");
        assert_eq!(oauth.priority, "high");
        let crash = items.iter().find(|i| i.filename == "fix-crash.md").unwrap();
        assert_eq!(crash.priority, "urgent");
        assert_eq!(crash.source, "crash");

        // Sanity: FakeFs's write counter shows exactly one write per
        // file (the insert_tree calls). Good regression guard for
        // "does my test accidentally double-write?"
        assert_eq!(fs.write_count(&inbox_dir.join("build-oauth.md")), 1);
        assert_eq!(fs.write_count(&inbox_dir.join("fix-crash.md")), 1);
    }

    #[test]
    fn fake_fs_simulates_missing_agent_work_dir() {
        use k2_core::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        // Intentionally do NOT scaffold the inbox — simulate a fresh
        // agent directory with no work yet. The caller must handle
        // NotFound gracefully.
        let err = fs
            .read_dir(Path::new("/proj/.k2so/agents/solo/work/inbox"))
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn fake_fs_verifies_frontmatter_round_trip_via_update_field() {
        // End-to-end: scaffold an AGENT.md in FakeFs, read it out,
        // pass through the extracted pure updater, write the result
        // back. Confirm the write went through and content matches.
        use k2_core::fs_abstract::{FakeFs, Fs};
        use std::path::Path;

        let fs = FakeFs::new();
        let agent_md = Path::new("/proj/.k2so/agents/rust-eng/AGENT.md");
        let original = "---\nrole: rust engineer\ntype: custom\n---\n# Rust engineer\n\nFocus: backend, systems.";
        fs.insert_file(agent_md, original.as_bytes());

        let content = fs.read_to_string(agent_md).unwrap();
        let updated = update_agent_md_field(&content, "role", "principal rust engineer").unwrap();
        fs.write(agent_md, updated.as_bytes()).unwrap();

        let final_content = fs.read_to_string(agent_md).unwrap();
        assert!(final_content.contains("role: principal rust engineer"));
        assert!(final_content.contains("type: custom"));
        assert!(final_content.contains("# Rust engineer"));
        // write_count should be 2: insert_file (1) + write (1).
        assert_eq!(fs.write_count(agent_md), 2);
    }
}
