//! Wake-prompt composers + per-agent wakeup.md resolution.
//!
//! When the heartbeat scheduler decides an agent should wake, the
//! launch path needs to turn that decision into the `--append-system-
//! prompt <string>` argument passed to `claude`. This module owns that
//! composition:
//!
//! - Per-type wakeup templates compiled into the binary
//!   ([`WAKEUP_TEMPLATE_WORKSPACE`] etc.).
//! - Filesystem resolution for the on-disk `WAKEUP.md` ([`read_agent_wakeup`]).
//! - Heartbeat-row-aware resolution ([`default_heartbeat_wakeup_abs`]).
//! - Pure composers ([`compose_manager_wake_from_body`], …) that take
//!   a raw body string and emit the full wake message. Split out so
//!   the branch coverage (body present / empty / missing) is unit-
//!   testable without scaffolding a filesystem.
//!
//! All entry points are Tauri-free so the daemon can call them
//! identically to the Tauri app.

use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::workspace::agent_identity::{agent_dir, agent_type_for, resolve_project_id};
use crate::db::schema::AgentHeartbeat;
use crate::terminal::event_sink::TerminalEventSink;
use crate::terminal::grid_types::GridUpdate;

// Shipped templates. Built-in markdown copied from
// `crates/k2so-core/wakeup_templates/*.md` at compile time. The
// matching files ship alongside this module; src-tauri's historical
// copies under `src-tauri/wakeup_templates/` are retained only for any
// reference callers that still want the UPPERCASE file scaffold — the
// authoritative sources now live here.

pub const WAKEUP_TEMPLATE_WORKSPACE: &str =
    include_str!("../../wakeup_templates/workspace.md");
pub const WAKEUP_TEMPLATE_MANAGER: &str =
    include_str!("../../wakeup_templates/manager.md");
pub const WAKEUP_TEMPLATE_CUSTOM: &str =
    include_str!("../../wakeup_templates/custom.md");
pub const WAKEUP_TEMPLATE_K2SO: &str = include_str!("../../wakeup_templates/k2so.md");

/// Resolve the shipped template for a given agent type. Returns `None`
/// for agent types that don't use wake-up at all (currently just
/// `agent-template`, which is always dispatched with explicit orders
/// by a manager).
pub fn wakeup_template_for(agent_type: &str) -> Option<&'static str> {
    match agent_type {
        "manager" | "coordinator" | "pod-leader" => Some(WAKEUP_TEMPLATE_MANAGER),
        "custom" => Some(WAKEUP_TEMPLATE_CUSTOM),
        "k2so" => Some(WAKEUP_TEMPLATE_K2SO),
        _ => None,
    }
}

/// Canonical location of an agent's WAKEUP.md on disk (UPPERCASE as of
/// 0.32.7).
pub fn agent_wakeup_path(project_path: &str, agent_name: &str) -> PathBuf {
    agent_dir(project_path, agent_name).join("WAKEUP.md")
}

/// Canonical location of the workspace-level WAKEUP.md (used by the
/// workspace's manager-mode primary agent).
pub fn workspace_wakeup_path(project_path: &str) -> PathBuf {
    crate::workspace_dot_dir(project_path).join("WAKEUP.md")
}

/// Read an agent's `WAKEUP.md`, falling back to the shipped template
/// if the file is missing or empty. Returns `None` for agent types
/// that don't use wake-up at all.
pub fn read_agent_wakeup(
    project_path: &str,
    agent_name: &str,
    agent_type: &str,
) -> Option<String> {
    let template = wakeup_template_for(agent_type)?;
    let path = agent_wakeup_path(project_path, agent_name);
    match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => Some(s),
        _ => Some(template.to_string()),
    }
}

/// Strip YAML frontmatter (`---`-delimited) from markdown content,
/// returning just the body. Body-less inputs (no closing fence) are
/// returned trimmed.
pub fn strip_frontmatter(content: &str) -> String {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            return content[3 + end + 3..].trim().to_string();
        }
    }
    content.trim().to_string()
}

/// Absolute path of the workspace's primary heartbeat `WAKEUP.md`.
/// Prefers a row named `"triage"` (the one
/// `migrate_or_scaffold_lead_heartbeat` creates for manager mode);
/// falls back to the first enabled row. `None` when the workspace has
/// no heartbeats configured — callers should fall back to the shipped
/// template in that case. Heartbeats are workspace-scoped, so no
/// agent-name parameter is needed.
pub fn default_heartbeat_wakeup_abs(project_path: &str) -> Option<String> {
    let db = crate::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, project_path)?;
    let rows = AgentHeartbeat::list_enabled(&conn, &project_id).ok()?;
    let hb = rows.iter().find(|h| h.name == "triage").or_else(|| rows.first())?;
    let abs = std::path::Path::new(project_path).join(&hb.wakeup_path);
    Some(abs.to_string_lossy().to_string())
}

/// Pure composer for the Workspace Manager wake prompt. Given an
/// optional raw wakeup body (as read from disk), returns the body
/// itself — frontmatter stripped, fallback to
/// [`WAKEUP_TEMPLATE_WORKSPACE`] if the body is empty or missing.
/// No "K2SO Heartbeat Wake" preamble — the wakeup.md is the message.
pub fn compose_manager_wake_from_body(raw_body: Option<&str>) -> String {
    raw_body
        .map(|s| strip_frontmatter(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| WAKEUP_TEMPLATE_WORKSPACE.trim().to_string())
}

/// Compose the `--append-system-prompt` text for the workspace's
/// manager-mode primary agent at wake time. Reads the wakeup body
/// from the workspace's default heartbeat row and falls back to the
/// shipped template if no row exists yet. The composition is keyed
/// on workspace identity — manager wakes don't need an agent name
/// parameter because the workspace has at most one manager-mode
/// primary by design.
pub fn compose_wake_prompt_for_workspace(project_path: &str) -> String {
    let raw = default_heartbeat_wakeup_abs(project_path)
        .and_then(|p| fs::read_to_string(&p).ok());
    compose_manager_wake_from_body(raw.as_deref())
}

/// Pure composer for a regular-agent wake prompt. Given the raw
/// wakeup body string (already read from disk), returns the body
/// itself — frontmatter stripped, no boilerplate prefix added.
///
/// Earlier versions wrapped the body in a "# K2SO Heartbeat Wake /
/// The heartbeat scheduler woke you..." preamble; this got
/// surfaced inside Claude's chat panel and felt like noise from
/// the user's perspective. The wakeup.md author already knows
/// they're writing the wake instructions — they don't need K2SO
/// re-explaining the context to them.
///
/// Returns `None` for: missing body input, OR body whose post-
/// frontmatter content is empty/whitespace. The empty-body case
/// matches a scaffolded-but-not-yet-edited WAKEUP.md — firing
/// claude with an empty prompt is a confusing no-op, so the
/// caller (smart_launch) records this as a fire-time error
/// instead of spawning.
pub fn compose_agent_wake_from_body(raw_body: Option<&str>) -> Option<String> {
    let body = raw_body?;
    let stripped = strip_frontmatter(body);
    if stripped.trim().is_empty() {
        return None;
    }
    Some(stripped)
}

/// Compose the `--append-system-prompt` text for a regular agent
/// woken by the heartbeat scheduler. Returns `None` for agent types
/// that don't have wake-up semantics (agent-template).
pub fn compose_wake_prompt_for_agent(
    project_path: &str,
    agent_name: &str,
) -> Option<String> {
    let agent_type = agent_type_for(project_path, agent_name);
    let wakeup = read_agent_wakeup(project_path, agent_name, &agent_type)?;
    compose_agent_wake_from_body(Some(&wakeup))
}

/// Compose the wake prompt from an explicit wakeup file path. Used by
/// the multi-heartbeat scheduler — each heartbeat row stores the path
/// it should read rather than relying on a naming convention.
pub fn compose_wake_prompt_from_path(
    wakeup_path: &std::path::Path,
) -> Option<String> {
    let content = std::fs::read_to_string(wakeup_path).ok()?;
    compose_agent_wake_from_body(Some(&content))
}

// ── Headless wake spawn ─────────────────────────────────────────────────
//
// Daemon-side entry point. The full `spawn_wake_pty` in
// src-tauri/agent_hooks.rs composes per-agent CLAUDE.md + inspects
// active worktrees + auto-saves the Claude session-ID 5 seconds later
// by scanning ~/.claude/projects. That machinery is Tauri-app territory.
//
// The daemon doesn't need any of it for lid-closed wakes. The agent
// just needs to launch `claude --dangerously-skip-permissions
// --append-system-prompt <body>` in the project directory, with a PTY
// so the TUI initializes. This helper is that minimal path.
//
// Design choices:
// - NoOp `TerminalEventSink`. Lid-closed wakes have no UI consumer;
//   when the Tauri app reopens, it learns about the running PTY via
//   `HookEvent::CliTerminalSpawnBackground` (which propagates through
//   the daemon's /events WS).
// - `k2so_agents_lock` marks the session `running` so the scheduler's
//   `is_agent_locked` check skips the agent on the next tick.
// - Sensible default terminal dims (120×38) match the existing nudge
//   size from src-tauri.
//
// This is intentionally simpler than `k2so_agents_build_launch` —
// no worktree resume, no inbox delegate. Those paths remain Tauri-
// only for v1; they involve user-facing decisions (new branches,
// work-item moves) that belong in supervised sessions.

/// `TerminalEventSink` impl that ignores every event. Lives alongside
/// the wake path because that's currently its only consumer; if more
/// host-less terminal use cases appear we can promote it to
/// `k2_core::terminal`.
pub struct NoOpTerminalEventSink;

impl TerminalEventSink for NoOpTerminalEventSink {
    fn on_title(&self, _terminal_id: &str, _title: &str) {}
    fn on_bell(&self, _terminal_id: &str) {}
    fn on_exit(&self, _terminal_id: &str, _exit_code: i32) {}
    fn on_grid_update(&self, _terminal_id: &str, _update: &GridUpdate) {}
}

/// Spawn a wake PTY headlessly from the daemon. Returns the generated
/// terminal ID on success.
///
/// Side effects in order:
/// 1. `TerminalManager::create` in `crate::terminal::shared()` — the
///    PTY is owned by the daemon process, survives the Tauri app
///    reopening/closing.
/// 2. [`crate::workspace::session::k2so_agents_lock`] — writes the
///    `agent_sessions` row + legacy `.lock` file so subsequent
///    scheduler ticks skip this agent.
/// 3. `AgentHookEventSink` fires `HookEvent::CliTerminalSpawnBackground`
///    — lets any connected Tauri UI create a tab for the new PTY via
///    the daemon's /events WebSocket.
// `heartbeat_name`: when Some, the wake is on behalf of a specific
// scheduled heartbeat. The deferred session-id save additionally
// writes to `agent_heartbeats.last_session_id` so the heartbeat keeps
// its own dedicated chat thread separate from the agent's global
// session. None = manual / awareness-bus / non-heartbeat wake.
// 0.37.0 RETIRED. The daemon-side wake fire moved to
// `k2_daemon::wake_headless::spawn_wake_headless` which routes
// through `spawn_agent_session_v2_blocking` (DaemonPtySession +
// v2_session_map). This function spawned through the in-process
// `terminal::shared()` `TerminalManager` (Alacritty Legacy
// backend) and is now dead code — kept for one release as a
// compile-time tombstone so any unexpected caller surfaces in
// CI before deletion. Slated for removal in 0.38.0.
#[allow(dead_code)]
#[deprecated(
    since = "0.37.0",
    note = "use k2_daemon::wake_headless::spawn_wake_headless (v2 backend)"
)]
pub fn spawn_wake_headless(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
    heartbeat_name: Option<&str>,
) -> Result<String, String> {
    // Opt-in trace for wake-spawn investigation. Set `K2SO_TRACE_WAKE_SPAWN=1` to enable.
    if std::env::var("K2SO_TRACE_WAKE_SPAWN").map(|v| v == "1").unwrap_or(false) {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!(
            "[wake-spawn-trace] spawn_wake_headless agent={agent_name:?} project={project_path:?} \
             heartbeat_name={heartbeat_name:?} prompt_len={}\n{bt}",
            wake_prompt.len()
        );
    }
    let terminal_id = format!("wake-{}-{}", agent_name, uuid::Uuid::new_v4());

    // Pre-allocate the Claude session UUID and pin it via
    // `--session-id`. Without this, two concurrent claude invocations
    // in the same project root attach to whichever session Claude
    // considers most recent — the deferred-save thread then stamps
    // both heartbeat rows with the same id (proven in production:
    // TestingK2SO fast-test + triage at 22:50/22:51 both got
    // f13453c8). Pinning the UUID at spawn time means each spawn
    // owns a distinct session id deterministically; we stamp the
    // row immediately, no race window, no async lookup heuristic.
    let pinned_session_id = uuid::Uuid::new_v4().to_string();

    // Daemon-spawned wakes use `--print` so claude delivers its
    // response and EXITS. Without this, every fresh fire leaves a
    // long-lived interactive claude PTY in the daemon's session
    // map, and cron's `find_live_for_resume` returns *that* ghost
    // PTY instead of the tab the user is watching — wakes go to a
    // hidden background process instead of the visible session.
    //
    // With --print:
    //   - Claude reads the positional prompt (the wakeup body)
    //   - Responds, persists the session jsonl (so --resume works)
    //   - Exits cleanly, PTY removed from session map
    //   - Next fire's find_live_for_resume sees only the user's tab
    //     (if open) and injects there, or spawns another short-lived
    //     -p claude (if no tab) — never a stale daemon PTY.
    //
    // Session persistence is on by default with --print (see
    // claude --help: --no-session-persistence is the OPPOSITE flag).
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--print".to_string(),
        "--session-id".to_string(),
        pinned_session_id.clone(),
        wake_prompt.to_string(),
    ];

    let sink: Arc<dyn TerminalEventSink> = Arc::new(NoOpTerminalEventSink);
    let manager = crate::terminal::shared();
    let mut manager = manager.lock();
    manager
        .create(
            terminal_id.clone(),
            project_path.to_string(),
            Some("claude".to_string()),
            Some(args),
            Some(120),
            Some(38),
            sink,
        )
        .map_err(|e| format!("spawn wake PTY: {e}"))?;
    drop(manager);

    crate::log_debug!(
        "[daemon/wake] spawned PTY for {} in {} (id={})",
        agent_name,
        project_path,
        terminal_id
    );

    // Mark the session running so the next scheduler tick skips it.
    // Best-effort: don't fail the spawn if the DB write trips (PTY is
    // already live and will run regardless).
    let _ = crate::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(terminal_id.clone()),
        Some("system".to_string()),
    );

    // Synchronous per-heartbeat session stamp. With --session-id
    // pinning above, we know exactly what session id Claude will
    // use, so we can write it to agent_heartbeats.last_session_id
    // immediately — no need to wait for the deferred-save thread to
    // poll claude history, no race between concurrent fires.
    //
    // Stamp `active_terminal_id` in the same critical section: this
    // is the FK-style pointer the renderer's openHeartbeatTab reads
    // to attach a new tab to the existing PTY (no fresh resume → no
    // duplicate session). See migration 0036 + the
    // `heartbeat-active-session-tracking` PRD.
    if let Some(hb_name) = heartbeat_name {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = crate::workspace::agent_identity::resolve_project_id(&conn, project_path) {
            let _ = crate::db::schema::AgentHeartbeat::save_session_id(
                &conn, &project_id, hb_name, &pinned_session_id,
            );
            let _ = crate::db::schema::AgentHeartbeat::save_active_terminal_id(
                &conn, &project_id, hb_name, &terminal_id,
            );
        }
        crate::log_debug!(
            "[daemon/wake] pinned heartbeat '{}' session id: {} terminal: {}",
            hb_name, pinned_session_id, terminal_id
        );
    }

    // Tell any connected UI. Wire format matches what the existing
    // src-tauri spawn_wake_pty emits so the React frontend's listener
    // doesn't need to branch on origin. The `heartbeatName` field
    // gates tab creation on the workspace's show_heartbeat_sessions
    // flag — silent autonomous heartbeats never surface a tab unless
    // the user opts in.
    crate::agent_hooks::emit(
        crate::agent_hooks::HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": &terminal_id,
            "command": "claude",
            "cwd": project_path,
            "heartbeatName": heartbeat_name,
            "projectPath": project_path,
            "agentName": agent_name,
        }),
    );

    // Deferred session-ID save: claude writes the session JSONL a few
    // seconds after launch; poll the provider's history dir and persist
    // the newest session on `agent_sessions.session_id` so the *next*
    // wake can `--resume` into the same chat. Best-effort, runs off a
    // detached thread so a slow filesystem never stalls the wake path.
    {
        let agent_name_owned = agent_name.to_string();
        let project_path_owned = project_path.to_string();
        let heartbeat_name_owned = heartbeat_name.map(str::to_string);
        // Capture spawn time so the deferred-save thread can match
        // its claude session id by *proximity to this spawn*, not
        // "newest session in the project". Without this, two
        // heartbeats firing on the same agent within a short window
        // both pick the same (highest-timestamp) session id and
        // stamp it on both rows. See `detect_claude_session_near`.
        let spawn_ms = chrono::Utc::now().timestamp_millis();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(5));
            // For heartbeat fires, use the proximity-matched lookup
            // so concurrent fires on the same agent don't clobber
            // each other. For non-heartbeat (Chat-tab) wakes the
            // legacy "newest session" semantics still apply — there
            // is no concurrency contention there.
            // Heartbeat fires now stamp last_session_id synchronously
            // at spawn time via --session-id pinning, so the deferred
            // poll is only needed for non-heartbeat (Chat-tab) wakes.
            if heartbeat_name_owned.is_some() {
                return;
            }
            let detected = crate::chat_history::detect_active_session(
                "claude", &project_path_owned,
            )
            .ok()
            .flatten();
            if let Some(session_id) = detected {
                if session_id.is_empty() {
                    return;
                }
                // Heartbeat fires must NOT touch agent_sessions.session_id
                // (the Chat tab's resume target). Each heartbeat's session
                // is independent of the user's Chat tab — see the
                // matching comment in src-tauri's spawn_wake_pty.
                if heartbeat_name_owned.is_none() {
                    match crate::workspace::session::k2so_agents_save_session_id(
                        project_path_owned.clone(),
                        agent_name_owned.clone(),
                        session_id.clone(),
                    ) {
                        Ok(_) => crate::log_debug!(
                            "[daemon/wake] saved session id for {}: {}",
                            agent_name_owned,
                            session_id
                        ),
                        Err(e) => crate::log_debug!(
                            "[daemon/wake] save session id for {} failed: {}",
                            agent_name_owned,
                            e
                        ),
                    }
                }

                // Per-heartbeat save (post-0.36.0). Each heartbeat
                // fire keeps its own chat thread so users can audit
                // each heartbeat independently from the Chat tab.
                if let Some(ref hb_name) = heartbeat_name_owned {
                    let db = crate::db::shared();
                    let conn = db.lock();
                    if let Some(project_id) = crate::workspace::agent_identity::resolve_project_id(&conn, &project_path_owned) {
                        match crate::db::schema::AgentHeartbeat::save_session_id(
                            &conn, &project_id, hb_name, &session_id,
                        ) {
                            Ok(_) => crate::log_debug!(
                                "[daemon/wake] saved heartbeat '{}' session id: {}",
                                hb_name, session_id
                            ),
                            Err(e) => crate::log_debug!(
                                "[daemon/wake] save heartbeat '{}' session id failed: {}",
                                hb_name, e
                            ),
                        }
                    }
                }
            }
        });
    }

    Ok(terminal_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_frontmatter_drops_fm_and_keeps_body() {
        let md = "---\nname: x\n---\n\n# body\ncontent\n";
        let body = strip_frontmatter(md);
        assert_eq!(body, "# body\ncontent");
    }

    #[test]
    fn strip_frontmatter_leaves_body_without_fm_alone() {
        assert_eq!(strip_frontmatter("# heading\n"), "# heading");
    }

    #[test]
    fn compose_manager_wake_falls_back_to_shipped_template_on_none() {
        // P8: composer no longer adds a "K2SO Heartbeat Wake — Workspace
        // Manager" preamble — the wakeup body itself is the message.
        // Falling back means the shipped WAKEUP_TEMPLATE_WORKSPACE
        // content appears verbatim. Assert the template's first line
        // is in the output as a structural sanity check.
        let composed = compose_manager_wake_from_body(None);
        assert!(!composed.is_empty(), "fallback should produce non-empty output");
        let template_lead = WAKEUP_TEMPLATE_WORKSPACE.trim().lines().next().unwrap_or("");
        assert!(!template_lead.is_empty());
        assert!(
            composed.contains(template_lead),
            "expected fallback to contain template's first line '{}', got: {}",
            template_lead, composed
        );
    }

    #[test]
    fn compose_manager_wake_strips_frontmatter() {
        let raw = "---\ndescription: blurb\n---\n\n# body\nkey text\n";
        let composed = compose_manager_wake_from_body(Some(raw));
        assert!(composed.contains("# body"));
        assert!(composed.contains("key text"));
        assert!(!composed.contains("description:"));
    }

    #[test]
    fn compose_agent_wake_returns_none_on_none_input() {
        assert!(compose_agent_wake_from_body(None).is_none());
    }

    #[test]
    fn compose_agent_wake_returns_body_verbatim() {
        // P8: composer returns body itself (frontmatter stripped),
        // no boilerplate preamble. The wakeup body IS the message.
        let composed = compose_agent_wake_from_body(Some("do the thing")).unwrap();
        assert!(composed.contains("do the thing"));
        assert!(!composed.contains("K2SO Heartbeat Wake"), "preamble retired in P8");
    }

    #[test]
    fn compose_agent_wake_strips_frontmatter() {
        let composed = compose_agent_wake_from_body(Some("---\ntag: x\n---\nbody")).unwrap();
        assert!(composed.contains("body"));
        assert!(!composed.contains("tag:"));
    }

    #[test]
    fn compose_agent_wake_returns_none_for_empty_body() {
        // Empty body (after frontmatter strip) is treated as
        // "nothing to wake with" — smart_launch records this as
        // a fire-time error instead of spawning claude with no prompt.
        assert!(compose_agent_wake_from_body(Some("---\ndescription:\n---\n\n")).is_none());
        assert!(compose_agent_wake_from_body(Some("   \n  ")).is_none());
    }

    #[test]
    fn wakeup_template_for_known_types() {
        assert!(wakeup_template_for("manager").is_some());
        assert!(wakeup_template_for("k2so").is_some());
        assert!(wakeup_template_for("custom").is_some());
        assert!(wakeup_template_for("agent-template").is_none());
        assert!(wakeup_template_for("bogus").is_none());
    }
}
