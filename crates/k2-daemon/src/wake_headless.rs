//! Daemon-side headless wake spawn (v2).
//!
//! Replaces the pre-0.37.0 `k2_core::agents::wake::spawn_wake_headless`,
//! which spawned through the in-process `terminal::shared()`
//! `TerminalManager` (Alacritty Legacy backend). 0.37.0 routes every
//! daemon-driven heartbeat fire through `spawn_agent_session_v2_blocking`
//! so the v2 invariants ("daemon-hosted, observable via grid WS,
//! attachable from Tauri later") hold for heartbeat fires too —
//! matching what every other daemon spawn already does (CLI launch,
//! awareness wake auto-launch, delegate worktree spawn).
//!
//! ## What stays the same
//!
//! - `claude --print --session-id <pinned>` semantics: heartbeat
//!   fires are short-lived one-shot invocations. claude reads the
//!   wake prompt, generates a response, persists session JSONL,
//!   exits. The PTY is removed from `v2_session_map` by the
//!   child-exit observer when claude returns.
//! - Pinned `--session-id` so two concurrent fires on the same
//!   agent get distinct deterministic UUIDs.
//! - Synchronous DB writes for heartbeat rows: `save_session_id` +
//!   `save_active_terminal_id` happen immediately after spawn, no
//!   deferred poll needed (the pinned UUID is what claude will use
//!   for its session JSONL).
//! - HookEvent emission so the frontend can react. Auto-surface is
//!   gated by the workspace's `show_heartbeat_sessions` flag —
//!   silent autonomous heartbeats never pop a tab unless the user
//!   has opted in. Once a tab attaches, `WorkspaceSession::set_surfaced`
//!   flips the per-row `surfaced` flag.
//!
//! ## What changes
//!
//! - Backend: `terminal::shared()` (in-process Alacritty Legacy) →
//!   `DaemonPtySession` via `spawn_agent_session_v2_blocking`.
//! - Map: legacy in-process `TerminalManager` registry → unified
//!   `v2_session_map`.
//! - terminal_id: previously a synthetic `wake-<agent>-<uuid>` string;
//!   now it's the v2 `SessionId` (also a UUID). Renderer attach paths
//!   (`openHeartbeatTab` reads `active_terminal_id`) remain compatible.
//! - Deferred-save thread retired for the heartbeat case (already a
//!   no-op there — pinned UUID makes the synchronous save authoritative).
//!   Non-heartbeat scheduler-tick fires (where `heartbeat_name` is None)
//!   still run a deferred save for compatibility with the legacy
//!   "save the chat-tab session id" semantic.

use std::sync::Arc;

use k2_core::log_debug;

use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// Daemon-side headless wake spawn. Returns the v2 session id as a
/// String (the renderer's `openHeartbeatTab` flow + the daemon's
/// `find_live_for_resume` both work against this id).
///
/// `heartbeat_name` distinguishes heartbeat fires (for which we
/// stamp `agent_heartbeats.last_session_id` + `active_terminal_id`
/// synchronously) from one-off chat-tab wakes (for which we run the
/// legacy deferred-save poll to capture whatever session id claude
/// happens to pick).
pub fn spawn_wake_headless(
    agent_name: &str,
    project_path: &str,
    wake_prompt: &str,
    heartbeat_name: Option<&str>,
) -> Result<String, String> {
    if std::env::var("K2SO_TRACE_WAKE_SPAWN")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!(
            "[wake-spawn-trace] spawn_wake_headless agent={agent_name:?} \
             project={project_path:?} heartbeat_name={heartbeat_name:?} \
             prompt_len={}\n{bt}",
            wake_prompt.len()
        );
    }

    // Pre-allocate claude's session UUID. Pinning via `--session-id`
    // means two concurrent fires on the same agent get distinct,
    // deterministic UUIDs — no race window between spawn and the
    // deferred-save thread guessing wrong.
    let pinned_session_id = uuid::Uuid::new_v4().to_string();

    // Interactive mode (no `--print`). The wakeup body is delivered
    // after spawn via a two-phase PTY write (body + 150ms settle +
    // `\r`), matching how `run_inject` writes wakeups to live PTYs
    // and how `pending_live::drain_for_agent` writes queued signals.
    // Why interactive instead of `--print`:
    //
    //   1. The PTY stays alive after the first fire, so subsequent
    //      fires hit `smart_launch`'s inject branch naturally — no
    //      `--print` ephemeral spawn that's invisible to the user.
    //   2. Opening the heartbeat tab from the sidebar reuses the
    //      same v2_session_map slot (idempotent attach via the
    //      canonical `<project_id>:<agent>` key), no duplicate
    //      `claude --resume` process.
    //   3. Audit-ability is preserved either way — claude writes the
    //      same JSONL whether run interactively or under `--print`.
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--session-id".to_string(),
        pinned_session_id.clone(),
    ];

    // Test-only override. Integration tests in
    // `crates/k2so-daemon/tests/heartbeat_fire_v2_integration.rs`
    // set this env var to a benign command (e.g. `cat`) so the
    // test can exercise the v2 spawn + post-spawn DB writes
    // without requiring `claude` on PATH or burning API calls.
    // Production never sets this — defaults to `claude` + the
    // args above.
    let (command, args) = match std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND") {
        Ok(c) if !c.is_empty() => (c, Vec::<String>::new()),
        _ => ("claude".to_string(), args),
    };

    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path)
    };

    // 0.37.8 — heartbeat fires get their own per-heartbeat canonical
    // key in `v2_session_map` so they don't collide with the chat tab's
    // bare-`<project_id>` slot. Pre-fix, `spawn_agent_session_v2_blocking`'s
    // idempotency check returned the chat tab's existing PTY for every
    // heartbeat fire — every wakeup ended up dropped into the chat tab
    // session and `workspace_heartbeats.active_terminal_id` was stamped
    // to the chat tab's PTY id. Per-heartbeat keys keep the lanes
    // separate; chat tab calls (heartbeat_name = None) keep the default.
    let canonical_key_override = match (project_id.as_deref(), heartbeat_name) {
        (Some(pid), Some(hb)) if !pid.is_empty() && !hb.is_empty() => {
            Some(format!("{pid}:hb:{hb}"))
        }
        _ => None,
    };

    let outcome = spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: project_id.clone(),
        cwd: project_path.to_string(),
        command: Some("claude".to_string()),
        args: Some(args),
        cols: 120,
        rows: 38,
        canonical_key: canonical_key_override,
    })?;

    let terminal_id = outcome.session_id.to_string();

    log_debug!(
        "[daemon/wake] spawned v2 PTY for {} in {} (id={})",
        agent_name,
        project_path,
        terminal_id
    );

    // Deliver the wakeup body to the freshly-spawned interactive
    // claude. Two-phase write — body, settle, `\r` — same pattern
    // run_inject uses on a live PTY. A single combined write lands
    // the body as a multi-line paste in the TUI input widget,
    // typed-but-not-sent. Claude's TUI needs ~1s to start up before
    // it accepts input cleanly, so wait a beat first.
    //
    // Skip when running under the test override (the test command
    // is e.g. `cat`, which doesn't need a wakeup payload).
    let is_test_override = std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND")
        .ok()
        .filter(|c| !c.is_empty())
        .is_some();
    if !is_test_override {
        // Look up the freshly-spawned session by its v2 session id.
        // It was just registered by `spawn_agent_session_v2_blocking`
        // a few microseconds ago.
        if let Some(session) =
            crate::v2_session_map::lookup_by_session_id(&outcome.session_id)
        {
            let prompt = wake_prompt.to_string();
            std::thread::spawn(move || {
                // Wait for claude TUI to draw its initial prompt.
                std::thread::sleep(std::time::Duration::from_millis(1500));
                session.write(prompt.into_bytes());
                std::thread::sleep(std::time::Duration::from_millis(150));
                session.write(b"\r".to_vec());
            });
        } else {
            log_debug!(
                "[daemon/wake] post-spawn lookup miss for session={} — wakeup body not delivered",
                terminal_id
            );
        }
    }

    // 0.37.8 — only chat-tab wakes (heartbeat_name = None) touch the
    // workspace_sessions row. Heartbeat fires live in their own lane
    // and must NOT clobber the chat tab's `active_terminal_id` /
    // `terminal_id` / `session_id`. Pre-fix this lock call ran
    // unconditionally and was the second contributor to the lane
    // collapse (along with the canonical_key collision).
    if heartbeat_name.is_none() {
        let _ = k2_core::workspace::session::k2so_agents_lock(
            project_path.to_string(),
            agent_name.to_string(),
            Some(terminal_id.clone()),
            Some("system".to_string()),
        );
    }

    // Synchronous per-heartbeat session stamp. With --session-id
    // pinning, we know exactly what UUID claude will use — write to
    // workspace_heartbeats.last_session_id immediately, no race.
    // active_terminal_id stamps the FK pointer that the renderer's
    // openHeartbeatTab uses to find the running PTY.
    if let Some(hb_name) = heartbeat_name {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(pid) = project_id.as_deref() {
            let _ = k2_core::db::schema::AgentHeartbeat::save_session_id(
                &conn, pid, hb_name, &pinned_session_id,
            );
            let _ = k2_core::db::schema::AgentHeartbeat::save_active_terminal_id(
                &conn, pid, hb_name, &terminal_id,
            );
            // #677.1 — heartbeat just went live (PTY attached).
            crate::session_events::emit_heartbeat_live("", pid, hb_name, true);
        }
        log_debug!(
            "[daemon/wake] pinned heartbeat '{}' session id: {} terminal: {}",
            hb_name,
            pinned_session_id,
            terminal_id
        );
    }

    // Emit a HookEvent so the frontend's listener can decide whether
    // to surface a tab. Gated by `projects.show_heartbeat_sessions`
    // on the renderer side — silent fires don't pop a tab unless
    // the user opted in. Same wire format the legacy spawn emitted
    // so existing subscribers don't need to branch.
    k2_core::agent_hooks::emit(
        k2_core::agent_hooks::HookEvent::CliTerminalSpawnBackground,
        serde_json::json!({
            "terminalId": &terminal_id,
            "command": "claude",
            "cwd": project_path,
            "heartbeatName": heartbeat_name,
            "projectPath": project_path,
            "agentName": agent_name,
        }),
    );

    // Deferred session-id save (non-heartbeat path only). For chat-tab
    // wakes (heartbeat_name = None), claude's session id wasn't pinned
    // synchronously — poll the chat history dir a few seconds later
    // and stamp `agent_sessions.session_id` for the next --resume.
    // For heartbeat fires the synchronous save above is authoritative.
    //
    // Factored into the shared `defer_stamp_adopted_session` helper so
    // `pinned_chat::ensure_pinned_chat` can reuse the SAME read-back to
    // make `workspace_sessions.session_id` the single source of truth
    // (pinned-chat-identity-ssot PRD §4.1a; GH#24). This call keeps the
    // wake path's behavior identical — same agent_name keying, same ~5s
    // window, same `save_session_id` persistence.
    if heartbeat_name.is_none() {
        defer_stamp_adopted_session(project_path.to_string(), agent_name.to_string());
    }

    // The Arc dropping silently retires the unused outcome metadata.
    let _ = Arc::new(outcome);

    Ok(terminal_id)
}

/// Deferred post-spawn read-back: stamp `workspace_sessions.session_id`
/// with the Claude session the spawned PTY **actually adopted on disk**.
///
/// This is the eager write-truth-at-the-source half of the pinned-chat
/// identity SSOT (see `.k2so/prds/pinned-chat-identity-ssot.md` §4.1a).
/// Claude writes its `.jsonl` a beat after spawn (the session id only
/// lands once the conversation persists), so we sleep ~5s on a detached
/// thread, probe the on-disk session dir via
/// `chat_history::newest_claude_session_on_disk(path)`, then persist the
/// discovered id through `WorkspaceSession::update_session_id`.
///
/// NOTE (smoke-test finding, GH#24): this used to call
/// `detect_active_session("claude", …)`, which keys on
/// `~/.claude/history.jsonl` — and that file has no entry for a freshly
/// spawned, not-yet-messaged chat within the ~5s window, so the eager
/// stamp was a silent no-op. `newest_claude_session_on_disk` scans
/// `~/.claude/projects/<hash>/*.jsonl` by mtime, so it catches the
/// `--session-id`-created file immediately and the stamp actually fires.
///
/// WHY this matters (GH#24): identity used to be argv-derived and
/// scattered across three stores, none of which recorded the id Claude
/// *actually* used. Stamping the adopted id here makes
/// `workspace_sessions.session_id` truthful at the source, so the
/// resolver's `claude_session_file_exists` happy-path hits on the next
/// resolve — no re-mint, no re-resume loop. The 0.39.40 resolver
/// converge fallback (`resume_chat.rs`) stays as the lazy safety net for
/// any path that misses this eager stamp.
///
/// Shared by the wake path (chat-tab wakes, `heartbeat_name = None`,
/// passing the real agent folder name) and
/// `pinned_chat::ensure_pinned_chat` (after a fresh `!reused` spawn,
/// passing the canonical `project_id` as the agent_name).
///
/// `agent_name` is used only for log context; the persist itself is
/// keyed on the `project_id` resolved from `project_path` (the
/// `workspace_sessions` row is `project_id`-unique). We deliberately do
/// **not** route through `k2so_agents_save_session_id` here: that helper
/// guards on an on-disk agent dir (`AGENT.md`/`SKILL.md`), which the
/// pinned-chat canonical key (`agent_name == project_id`) has no reason
/// to own. Persisting via `update_session_id` keyed on `project_id` is
/// behavior-equivalent for the wake path (a registered workspace always
/// has the row) and correct for the pinned chat (identity lives in that
/// one row — the SSOT).
///
/// Fire-and-forget: errors are logged, never surfaced — the lazy
/// resolver self-heal (`resume_chat.rs`, 0.39.40) covers a miss.
pub fn defer_stamp_adopted_session(project_path: String, agent_name: String) {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(5));
        let detected =
            k2_core::chat_history::newest_claude_session_on_disk(&project_path);
        let Some(session_id) = detected else { return };
        if session_id.is_empty() {
            return;
        }
        let db = k2_core::db::shared();
        let conn = db.lock();
        let Some(project_id) =
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &project_path)
        else {
            log_debug!(
                "[daemon/wake] save session id for {} skipped: unregistered project {}",
                agent_name,
                project_path
            );
            return;
        };
        match k2_core::db::schema::WorkspaceSession::update_session_id(
            &conn,
            &project_id,
            &session_id,
        ) {
            Ok(_) => log_debug!(
                "[daemon/wake] saved session id for {}: {}",
                agent_name,
                session_id
            ),
            Err(e) => log_debug!(
                "[daemon/wake] save session id for {} failed: {}",
                agent_name,
                e
            ),
        }
    });
}
