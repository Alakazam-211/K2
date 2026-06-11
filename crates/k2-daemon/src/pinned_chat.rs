//! Daemon-owned pinned-chat session lifecycle — Phase 1 (0.39.39).
//!
//! Authoritative spec: `.k2so/prds/daemon-owned-pinned-chat.md`.
//!
//! ## The one endpoint (decision D1)
//!
//! `POST /cli/workspace/ensure-pinned-chat` is the single idempotent
//! entrypoint a thin client calls to attach to (or create) a
//! workspace's canonical pinned-chat Claude session. It collapses the
//! pre-0.39.39 multi-step renderer dance —
//!
//!   1. `GET /cli/workspace/resume-chat-args` → resolve `claude` argv
//!   2. `POST /cli/sessions/v2/spawn` → spawn the PTY (renderer-minted
//!      `agent_name`, racy because two windows can mint different keys)
//!
//! — into a single atomic find-or-spawn keyed on the canonical
//! `<project_id>` slot (`canonical_session::canonical_key_for`). The
//! atomicity is what kills the `--session-id already in use` race
//! (#682): the resolver pre-allocates / persists the Claude session id
//! BEFORE the spawn, and `spawn_agent_session_v2_blocking` registers
//! the live PTY under the canonical key BEFORE this function returns —
//! so a second concurrent `ensure` finds the live session and reuses
//! it (`reused: true`) instead of spawning a duplicate that would try
//! to claim the same `--session-id`.
//!
//! ## What this does NOT touch
//!
//! - The existing `/cli/workspace/resume-chat-args`,
//!   `/cli/sessions/v2/spawn`, `/cli/workspace/set-chat-session`
//!   routes stay live — the renderer keeps using its current path
//!   until a later phase cuts it over behind a capability gate. This
//!   module ADDS alongside.
//! - No host-switch logic. The daemon is the daemon; remote-host
//!   selection is a thin-client concern (K2 Connect), not a daemon
//!   concern.
//! - No multi-subscriber grid-WS (decision D4: single-subscriber).
//!
//! ## Reuse, not duplication
//!
//! - Claude argv + the resume-vs-fresh decision: delegated wholesale
//!   to `k2_core::workspace::resume_chat::resolve_resume_chat_args`
//!   (returns command / args / cwd / resumeSession / resumedExisting;
//!   `--resume <id>` for a real prior session, `--session-id <new>`
//!   for a never-chatted workspace).
//! - Find-or-spawn under the canonical key: delegated to
//!   `crate::spawn::spawn_agent_session_v2_blocking` (the same v2 path
//!   `/cli/sessions/v2/spawn` and the wake provider use). Its
//!   idempotency check returns the existing live session with
//!   `reused=true`; its `register` call emits `SessionEvent::SessionAdded`
//!   over the `session_events` bus for free.

use crate::awareness_ws::HandlerResult;
use crate::canonical_session::canonical_key_for;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};
use k2_core::log_debug;
use k2_core::workspace::resume_chat::resolve_resume_chat_args_ex;

/// Resolved outcome of `ensure_pinned_chat`. Returned to the HTTP
/// handler (and exercised directly by the integration tests) so
/// callers get the canonical IDs + the launch config the resolver
/// produced.
#[derive(Debug, Clone)]
pub struct EnsurePinnedChatOutcome {
    /// Daemon-side v2 PTY session id (UUID string). This is the
    /// `active_terminal_id` a subsequent attach resolves to.
    pub session_id: String,
    /// The Claude conversation UUID we're resuming OR that was
    /// pre-allocated for a never-chatted workspace. Always set.
    pub claude_session_id: String,
    /// `true` when the saved Claude session was usable (JSONL on
    /// disk) → `--resume`. `false` when we pre-allocated a fresh UUID
    /// → `--session-id`.
    pub resumed_existing: bool,
    /// Spawn command (`claude`).
    pub command: String,
    /// Spawn args (`--dangerously-skip-permissions` + the resume /
    /// session-id pair).
    pub args: Vec<String>,
    pub cols: u16,
    pub rows: u16,
    /// `true` when a live canonical session already existed and was
    /// returned (no fresh spawn). `false` on a fresh spawn (cold
    /// workspace OR `force_respawn`).
    pub reused: bool,
}

impl EnsurePinnedChatOutcome {
    /// Wire shape (camelCase) for the HTTP response.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "sessionId": self.session_id,
            "claudeSessionId": self.claude_session_id,
            "resumedExisting": self.resumed_existing,
            "command": self.command,
            "args": self.args,
            "cols": self.cols,
            "rows": self.rows,
            "reused": self.reused,
        })
    }
}

/// Default PTY dimensions for a freshly-spawned pinned chat. The
/// renderer ResizeObserver-corrects on attach, so these only matter
/// for headless / mobile consumers until the first resize.
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Idempotent find-or-spawn of a workspace's canonical pinned-chat
/// session. See the module doc for the full contract.
///
/// - `project_path`: filesystem path to the workspace (also the
///   `projects.path` lookup key and the spawned PTY's cwd).
/// - `force_respawn`: when `true`, unregister any existing live
///   canonical session (which tears down its PTY and emits
///   `SessionRemoved`) BEFORE spawning a fresh one. When `false`
///   (the default), a live session is returned as-is (`reused: true`).
/// - `explicit_selection`: `true` ONLY when the caller is an explicit
///   user dropdown session pick (Issue B). Threaded into the resolver
///   so the chosen `workspace_sessions.session_id` is honored directly
///   and the GH#24 converge fallback is skipped — an explicit pick is
///   never silently reverted to the newest on-disk session. `false` for
///   normal cold mounts / refreshes (auto path, fallback intact).
///
/// Errors:
/// - `"project not registered: <path>"` — `project_path` doesn't
///   match any `projects.path` row.
/// - `"<resolver error>"` — the resolver failed on a DB I/O error, OR
///   (when `explicit_selection`) the explicitly-chosen session id has no
///   conversation on disk for this workspace.
/// - `"spawn failed: <e>"` — the underlying PTY spawn failed.
pub fn ensure_pinned_chat(
    project_path: &str,
    force_respawn: bool,
    explicit_selection: bool,
) -> Result<EnsurePinnedChatOutcome, String> {
    // 1. Resolve project_id. The canonical key is the bare project_id
    //    (canonical_session::canonical_key_for) — the same slot the
    //    pinned chat tab, the wake provider, and ensure-canonical-session
    //    all converge on.
    let project_id = lookup_project_id(project_path)
        .ok_or_else(|| format!("project not registered: {project_path}"))?;
    let canonical_key = canonical_key_for(&project_id);

    // 2. Resolve the Claude argv + the resume-vs-fresh decision. This
    //    is the chokepoint that pre-allocates + persists a fresh
    //    `--session-id <new>` (for a never-chatted workspace) BEFORE
    //    we spawn, or splices `--resume <id>` for a real prior
    //    session. DO NOT duplicate this logic.
    let resolved = resolve_resume_chat_args_ex(project_path, explicit_selection)?;

    // 3. force_respawn: tear down the existing live session first so
    //    the new spawn isn't short-circuited by the idempotency check
    //    inside spawn_agent_session_v2_blocking. unregister() emits
    //    SessionEvent::SessionRemoved + runs the active-session DB
    //    cleanup chokepoint, and drops the last map Arc → the PTY's
    //    child receives SIGHUP. The fresh spawn below then emits
    //    SessionAdded — net: Removed-then-Added on force_respawn.
    if force_respawn {
        if let Some(existing) = crate::v2_session_map::lookup_by_agent_name(&canonical_key) {
            log_debug!(
                "[daemon/pinned-chat] force_respawn: unregistering live canonical_key={} (session={})",
                canonical_key,
                existing.session_id,
            );
            crate::v2_session_map::unregister(&canonical_key);
            // Belt-and-suspenders: the last map Arc just dropped, but
            // the WS subscriber may still hold one. kill() is
            // idempotent + best-effort and guarantees the child's
            // teardown so the fresh spawn doesn't collide on the
            // resumed --session-id.
            existing.kill();
        }
    }

    // 4. Idempotent find-or-spawn under the canonical key. agent_name
    //    == project_id so the helper's `canonical_key_for(project_id)`
    //    derivation lands on the bare-`<project_id>` slot. If a live
    //    session exists (and !force_respawn — we just unregistered it
    //    otherwise) the helper returns it with reused=true and does NOT
    //    spawn a duplicate. The register() inside it emits SessionAdded
    //    + stamps workspace_tab_sessions, and registration completes
    //    before this returns — so a concurrent ensure reuses the same
    //    session id (closes the #682 dup-in-use race).
    let req = SpawnWorkspaceSessionRequest {
        agent_name: project_id.clone(),
        project_id: Some(project_id.clone()),
        cwd: resolved.cwd.clone(),
        command: Some(resolved.command.clone()),
        args: Some(resolved.args.clone()),
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        canonical_key: Some(canonical_key.clone()),
    };
    let spawn_outcome =
        spawn_agent_session_v2_blocking(req).map_err(|e| format!("spawn failed: {e}"))?;
    let session_id = spawn_outcome.session_id.to_string();

    // 5. Stamp workspace_sessions.active_terminal_id (so the next
    //    attach resolves the live PTY without walking the in-memory
    //    map) + bump last_activity_at. save_active_terminal_id touches
    //    only active_terminal_id, so we bump last_activity_at
    //    explicitly. Best-effort: a never-registered workspace_sessions
    //    row no-ops (the resolver's fresh-session branch INSERTs one,
    //    so the row exists after step 2 for the cold-workspace case).
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = k2_core::db::schema::WorkspaceSession::save_active_terminal_id(
            &conn,
            &project_id,
            &session_id,
        );
        let _ = conn.execute(
            "UPDATE workspace_sessions SET last_activity_at = unixepoch() WHERE project_id = ?1",
            rusqlite::params![project_id],
        );
    }

    // 6. Eager post-spawn read-back (pinned-chat-identity-ssot PRD §4.1a;
    //    GH#24). On a FRESH spawn (`!reused`), claude will write its
    //    `.jsonl` a beat from now under the session id it actually adopts.
    //    Defer ~5s, probe the chat-history dir, and stamp
    //    `workspace_sessions.session_id` (the SSOT) with the adopted id.
    //    This writes truth at the source so the resolver's
    //    `claude_session_file_exists` happy-path hits on the next ensure —
    //    no re-mint loop. On a reused PTY we skip: identity is already
    //    correct (the resolver/earlier read-back set it) and no new
    //    `.jsonl` is being created. The 0.39.40 resolver converge fallback
    //    remains the lazy safety net if this eager stamp ever misses.
    if !spawn_outcome.reused {
        crate::wake_headless::defer_stamp_adopted_session(
            project_path.to_string(),
            project_id.clone(),
        );
    }

    log_debug!(
        "[daemon/pinned-chat] ensured session={} canonical_key={} claude_session={} resumed_existing={} reused={} force_respawn={} explicit_selection={}",
        session_id,
        canonical_key,
        resolved.resume_session,
        resolved.resumed_existing,
        spawn_outcome.reused,
        force_respawn,
        explicit_selection,
    );

    Ok(EnsurePinnedChatOutcome {
        session_id,
        claude_session_id: resolved.resume_session,
        resumed_existing: resolved.resumed_existing,
        command: resolved.command,
        args: resolved.args,
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        reused: spawn_outcome.reused,
    })
}

/// `projects.id` lookup by `projects.path`. `None` when the workspace
/// isn't registered. Mirror of `canonical_session::lookup_project_id`.
fn lookup_project_id(project_path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Handler for `POST /cli/workspace/ensure-pinned-chat`.
///
/// Request body (JSON):
/// ```json
/// { "project": "/path/to/workspace", "forceRespawn": false }
/// ```
///
/// Response body (JSON, camelCase):
/// ```json
/// {
///   "sessionId": "<daemon-pty-uuid>",
///   "claudeSessionId": "<claude-conversation-uuid>",
///   "resumedExisting": true,
///   "command": "claude",
///   "args": ["--dangerously-skip-permissions", "--resume", "<id>"],
///   "cols": 80,
///   "rows": 24,
///   "reused": true
/// }
/// ```
pub fn handle_ensure_pinned_chat(body: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Req {
        project: String,
        #[serde(default)]
        force_respawn: bool,
        /// Issue B: set ONLY by an explicit dropdown session pick. When
        /// true the resolver honors the saved session_id directly and
        /// skips the GH#24 converge fallback. Optional (defaults false)
        /// so cold mounts / refreshes keep the auto path.
        #[serde(default)]
        explicit_selection: bool,
    }

    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse ensure-pinned-chat request: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    if req.project.is_empty() {
        return HandlerResult {
            status: "400 Bad Request",
            body: r#"{"error":"project required"}"#.into(),
        };
    }

    match ensure_pinned_chat(&req.project, req.force_respawn, req.explicit_selection) {
        Ok(out) => HandlerResult {
            status: "200 OK",
            body: out.to_json().to_string(),
        },
        Err(e) => HandlerResult {
            status: "400 Bad Request",
            body: serde_json::json!({ "error": e }).to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_request_rejects_empty_project() {
        let body = br#"{"project":""}"#;
        let result = handle_ensure_pinned_chat(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("project required"));
    }

    #[test]
    fn ensure_request_rejects_malformed_json() {
        let body = b"not json";
        let result = handle_ensure_pinned_chat(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("parse ensure-pinned-chat request"));
    }

    #[test]
    fn force_respawn_defaults_false_when_absent() {
        // Deserialization smoke test: a body without forceRespawn must
        // parse (serde default) rather than 400. We can't run the full
        // spawn here (no DB / no claude), so we assert the parse path
        // gets past the request-shape gate by checking the error is
        // about project registration, not JSON parsing.
        let body = br#"{"project":"/nonexistent/k2so-pinned-chat-unit-test"}"#;
        let result = handle_ensure_pinned_chat(body);
        // No claude binary / unregistered project → 400 with a
        // registration error, NOT a parse error.
        assert_eq!(result.status, "400 Bad Request");
        assert!(
            result.body.contains("project not registered"),
            "expected a registration error, got: {}",
            result.body
        );
    }
}
