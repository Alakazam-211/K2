//! Canonical-session ensurance — proactive spawn + DB row registration
//! when a workspace has a primary agent declared but no live session yet.
//!
//! ## The race this solves
//!
//! Pre-0.37.2, the only path that spawned a workspace's canonical PTY
//! was `DaemonWakeProvider::wake` (when a wake signal arrived) or the
//! renderer's AgentChatPane mount (when the user opened a chat tab).
//! For automation deployments — Baden's nsi-checkin Scout SMS bridge
//! is the canonical example — a fresh workspace flow is:
//!
//!   k2so workspace open <path>
//!   k2so mode custom
//!   # write .k2so/agent/AGENT.md
//!   <webhook fires k2so msg <ws> --wake within ~150ms>
//!
//! The daemon knows the workspace has a primary agent the moment
//! AGENT.md is written, but no canonical PTY exists and no
//! `workspace_sessions` row is registered yet. The webhook's `--wake`
//! falls into the smart-launch cascade's `fresh_fire` branch, which
//! spawns a session — but the spawn registers the SQL row as a side
//! effect rather than being preceded by it. Two symptoms:
//!
//! 1. The renderer's pinned-tab attach can race against `--wake`'s
//!    spawn and end up subscribing to a different session than the
//!    one the inject lands in. Same "blank pinned tab" class as the
//!    pre-0.37 Dannon Erskins bug.
//! 2. The first inject's audit row buckets to `_orphan` because the
//!    workspace+agent pair isn't registered when the egress path
//!    runs. Self-corrects on next spawn but leaves an audit gap.
//!
//! ## What this module does
//!
//! `ensure_canonical_session(project_path)` is the single, idempotent
//! entry point that:
//!
//! 1. Resolves the workspace's primary agent via
//!    `find_primary_agent` (post-unification: read
//!    `.k2so/agent/AGENT.md`'s `name:` field).
//! 2. Single-flight check: if a v2 session is already registered
//!    under canonical key `<project_id>:<agent>`, return early as
//!    `reused: true`.
//! 3. Loads the agent's launch profile (`.k2so/agent/AGENT.md`'s
//!    `launch:` YAML block) or falls back to the default profile
//!    (`claude --dangerously-skip-permissions` at project root).
//! 4. Spawns via `spawn_agent_session_v2_blocking` under the
//!    canonical key — guaranteed to register in `v2_session_map`
//!    before this function returns.
//! 5. Persists the workspace_sessions row via `k2so_agents_lock`,
//!    setting `terminal_id` to the v2 session id and marking the
//!    row `running` with `owner=system`.
//!
//! Called from:
//!
//! - `/cli/mode` set handler: when the operator/script sets the
//!   workspace's `agent_mode` to a bot mode (`custom`, `manager`,
//!   `k2so`), proactively ensure the canonical session exists.
//! - `/cli/workspace/ensure-canonical-session` HTTP endpoint:
//!   explicit caller-driven path. Replaces the SMS bridge's
//!   `agents launch <name>` workaround with a more semantically
//!   correct call that returns the canonical IDs the caller can
//!   use for follow-up inject/wake operations.
//! - Daemon boot sweep: at startup, walks every workspace whose
//!   `agent_mode` is set to a bot mode and calls ensure for each
//!   that doesn't have a live canonical session. Recovers cleanly
//!   from a daemon restart.
//! - `DaemonWakeProvider::wake` auto-launch path: refactored to
//!   call this helper rather than duplicating the spawn-and-register
//!   logic. Wake and proactive ensure converge on the same code.

use k2_core::workspace::launch_profile::{load_launch_profile, resolve_cwd, LaunchProfile};
use k2_core::log_debug;

use crate::session_lookup;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// Build the canonical `v2_session_map` key for a workspace.
///
/// **0.37.5 contract:** the canonical session for a workspace is keyed
/// purely on `project_id`. The agent's name is metadata about the
/// session (display label, persona file, launch profile owner) — not
/// part of its address.
///
/// Pre-0.37.5 the key was `<project_id>:<agent_name>`. That suffix
/// was vestigial — pre-unification when multiple agents lived per
/// workspace it was needed for disambiguation, but post-0.37.0
/// unification there's at most one agent per workspace and the
/// suffix only created opportunities for the renderer to compute
/// the wrong name (see C3PO `5c80bef1`: pinned-tab open hardcoded
/// `__lead__` for `mode custom`, missed the `<pid>:scout` slot,
/// spawned a duplicate PTY, orphaned scout).
///
/// Every consumer (`v2_session_map::register`, `lookup_by_agent_name`,
/// `pending_live` queue dirs, `workspace_sessions.terminal_id`) goes
/// through this helper to get the canonical key. One chokepoint, one
/// shape, no string assembly scattered across the codebase.
pub fn canonical_key_for(project_id: &str) -> String {
    project_id.to_string()
}

/// Result of `ensure_canonical_session`. Returned to callers (CLI,
/// boot sweep, wake provider) so they can log + take follow-up
/// action with the canonical IDs.
#[derive(Debug, Clone)]
pub struct EnsureOutcome {
    /// v2 PTY session id (UUID string). The renderer's
    /// `attachAgentName=<project_id>:<agent>` path resolves to this
    /// session on subsequent attaches.
    pub session_id: String,
    /// Workspace's primary agent name (read from AGENT.md frontmatter).
    pub agent_name: String,
    /// `projects.id` for the workspace.
    pub project_id: String,
    /// `true` if the canonical session was already alive — no spawn
    /// happened, the existing entry was returned. `false` on a fresh
    /// spawn.
    pub reused: bool,
    /// Number of pending-live signals drained into the freshly-spawned
    /// session. Always 0 on `reused: true`.
    pub pending_drained: usize,
}

/// Idempotent: spawn the workspace's canonical PTY and register
/// `workspace_sessions` if not already done. Skipped (returns
/// `reused: true`) when an entry exists in `v2_session_map` under
/// `<project_id>:<agent>`. See module doc for the full flow.
///
/// Errors:
/// - `"project not registered"` — `project_path` doesn't match any
///   `projects.path` row. Caller should register the workspace first.
/// - `"no primary agent in workspace"` — `find_primary_agent`
///   returned None (no AGENT.md, malformed AGENT.md, etc.).
/// - `"launch profile parse failed: ..."` — AGENT.md's `launch:`
///   block had invalid YAML. Use the default profile by removing
///   the block.
/// - `"spawn failed: ..."` — the underlying PTY spawn failed (bad
///   command, permission denied, etc.).
pub fn ensure_canonical_session(project_path: &str) -> Result<EnsureOutcome, String> {
    // 1. Resolve project_id + primary agent.
    let project_id = lookup_project_id(project_path)
        .ok_or_else(|| format!("project not registered: {project_path}"))?;
    let agent_name = k2_core::workspace::agent_identity::find_primary_agent(project_path)
        .ok_or_else(|| "no primary agent in workspace".to_string())?;

    // 2. Single-flight: if canonical session is already live, return.
    //    0.37.5: canonical key is bare project_id (see canonical_key_for
    //    doc-block). Drops the `:<agent_name>` suffix that previously
    //    encoded the agent's name into the address.
    let canonical_key = canonical_key_for(&project_id);
    if let Some(live) = session_lookup::lookup_any(&canonical_key) {
        let session_id = live.session_id().to_string();
        // Keep workspace_sessions.active_terminal_id pointed at the live
        // canonical PTY so the companion's isMainChat (and a later attach)
        // resolve to it — parity with the fresh-spawn path below.
        stamp_active_terminal_id(&project_id, &session_id);
        return Ok(EnsureOutcome {
            session_id,
            agent_name,
            project_id,
            reused: true,
            pending_drained: 0,
        });
    }

    // 3. Load launch profile (or default).
    let mut profile = match load_launch_profile(project_path, &agent_name) {
        Ok(Some(p)) => p,
        Ok(None) => default_launch_profile(),
        Err(e) => return Err(format!("launch profile parse failed: {e}")),
    };

    // 3b. Resume-aware canonical claude chat. A bare
    //     `claude --dangerously-skip-permissions` spawned here on a daemon
    //     restart / boot-sweep starts a FRESH conversation; because this PTY
    //     registers under the canonical key FIRST, a later
    //     `ensure-pinned-chat` finds it live and reuses it (`reused=true`),
    //     never reaching its own `--resume` resolver — so the workspace's
    //     selected session only came back on a manual reload (force_respawn).
    //     Resolve the resume args HERE, the SAME way the interactive
    //     pinned-chat path does, so the boot/restart respawn restores the
    //     selected session. We pass explicit_selection=TRUE to make this
    //     NON-DESTRUCTIVE: a valid saved session is resumed, but a
    //     saved-but-unresolvable one returns Err (we fall back to the bare
    //     profile and spawn fresh) WITHOUT overwriting
    //     workspace_sessions.session_id — so a proactive boot spawn can never
    //     clobber the user's pinned-chat pick with a throwaway mint. (The
    //     interactive reload force-respawns with explicit=false to converge to
    //     the newest conversation for continuity when the user asks.) Cold
    //     workspaces with no saved session still get a stable pinned id.
    //     Claude chats only; custom launch commands keep their profile args.
    let is_claude_chat = profile
        .command
        .as_deref()
        .and_then(|c| c.rsplit('/').next())
        .map(|name| name == "claude")
        .unwrap_or(false);
    if is_claude_chat {
        match k2_core::workspace::resume_chat::resolve_resume_chat_args_ex(project_path, true) {
            Ok(resolved) => {
                log_debug!(
                    "[daemon/canonical] resume-aware spawn for {project_path}: \
                     session={} resumed_existing={}",
                    resolved.resume_session,
                    resolved.resumed_existing,
                );
                profile.args = Some(resolved.args);
            }
            Err(e) => log_debug!(
                "[daemon/canonical] resume resolve failed for {project_path}, \
                 using profile args as-is: {e}"
            ),
        }
    }

    // 4. Spawn via the canonical-keyed v2 helper.
    let req = launch_request_for(&agent_name, &project_id, project_path, &profile);
    let outcome = spawn_agent_session_v2_blocking(req)
        .map_err(|e| format!("spawn failed: {e}"))?;

    // 5. Persist workspace_sessions row. Best-effort — the PTY is
    //    alive regardless. Mirrors what `try_auto_launch` and the
    //    `/cli/agents/launch` handler do.
    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.clone(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    // Point active_terminal_id at the freshly-spawned canonical PTY so the
    // companion's isMainChat (and a later attach) resolve to THIS live PTY
    // after a restart — the boot-sweep path previously skipped this, leaving a
    // stale pointer to the dead pre-restart PTY (pinned chat lost its identity).
    stamp_active_terminal_id(&project_id, &outcome.session_id.to_string());

    log_debug!(
        "[daemon/canonical] ensured session={} agent={} workspace={} \
         pending_drained={} (fresh spawn)",
        outcome.session_id,
        agent_name,
        project_id,
        outcome.pending_drained,
    );

    Ok(EnsureOutcome {
        session_id: outcome.session_id.to_string(),
        agent_name,
        project_id,
        reused: false,
        pending_drained: outcome.pending_drained,
    })
}

/// Default launch profile for workspaces whose AGENT.md doesn't
/// declare a `launch:` block. Mirrors the everyday "open the chat
/// tab in Tauri" command — claude in interactive mode at the
/// project root.
pub fn default_launch_profile() -> LaunchProfile {
    LaunchProfile {
        command: Some("claude".to_string()),
        args: Some(vec!["--dangerously-skip-permissions".to_string()]),
        cwd: None,
        cols: None,
        rows: None,
        env: Default::default(),
    }
}

/// Build a `SpawnWorkspaceSessionRequest` from a `LaunchProfile`.
/// Defaults applied here match `POST /cli/sessions/spawn` for
/// consistency with the explicit-spawn path.
pub fn launch_request_for(
    agent: &str,
    workspace_id: &str,
    project_path: &str,
    profile: &LaunchProfile,
) -> SpawnWorkspaceSessionRequest {
    let project_root = std::path::Path::new(project_path);
    let cwd = resolve_cwd(project_root, profile.cwd.as_deref())
        .to_string_lossy()
        .into_owned();
    SpawnWorkspaceSessionRequest {
        agent_name: agent.to_string(),
        project_id: Some(workspace_id.to_string()),
        cwd,
        command: profile.command.clone(),
        args: profile.args.clone(),
        cols: profile.cols.unwrap_or(80),
        rows: profile.rows.unwrap_or(24),
        canonical_key: None,
    }
}

/// `projects.id` lookup by `projects.path`. Returns None when the
/// workspace isn't registered.
pub fn lookup_project_id(project_path: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1",
        rusqlite::params![project_path],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Stamp `workspace_sessions.active_terminal_id` to the live canonical PTY.
/// The companion derives `isMainChat` by matching a session's PTY uuid
/// against this column; the boot-sweep spawn path historically skipped it
/// (only `pinned_chat.rs` set it), so after a restart it pointed at the dead
/// pre-restart PTY and the pinned chat lost its "main chat" identity.
/// Best-effort: a missing row no-ops (the fresh-spawn path's
/// `k2so_agents_lock` creates the row first).
fn stamp_active_terminal_id(project_id: &str, terminal_id: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let _ = k2_core::db::schema::WorkspaceSession::save_active_terminal_id(
        &conn,
        project_id,
        terminal_id,
    );
}

/// Boot-time sweep: walk every workspace whose `agent_mode` is set
/// to a bot mode (`custom`, `manager`, `k2so`) and AGENT.md exists,
/// and ensure each has a canonical session. Recovers cleanly from
/// a daemon restart that wiped `v2_session_map`.
///
/// Best-effort per workspace: a failure on one workspace doesn't
/// stop the sweep from continuing to the next.
pub fn boot_sweep_ensure_canonical_sessions() {
    let projects: Vec<(String, String)> = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut stmt = match conn.prepare(
            "SELECT id, path FROM projects \
             WHERE agent_mode IN ('custom', 'manager', 'k2so')",
        ) {
            Ok(s) => s,
            Err(_) => return,
        };
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        });
        match rows {
            Ok(it) => it.flatten().collect(),
            Err(_) => return,
        }
    };

    if projects.is_empty() {
        return;
    }

    let mut ensured = 0usize;
    let mut reused = 0usize;
    let mut errors = 0usize;
    for (_pid, path) in &projects {
        // Skip workspaces whose filesystem dir is gone (deleted on
        // disk but still in the DB).
        if !std::path::Path::new(path).exists() {
            continue;
        }
        // Skip workspaces without an AGENT.md (mode set but agent
        // not yet authored). The boot sweep should not implicitly
        // synthesize an agent.
        let agent_md = k2_core::workspace_dot_dir(path).join("agent/AGENT.md");
        if !agent_md.exists() {
            continue;
        }
        match ensure_canonical_session(path) {
            Ok(out) if out.reused => reused += 1,
            Ok(_) => ensured += 1,
            Err(e) => {
                log_debug!(
                    "[daemon/canonical] boot sweep skipped workspace {path}: {e}"
                );
                errors += 1;
            }
        }
    }

    if ensured + reused + errors > 0 {
        log_debug!(
            "[daemon/canonical] boot sweep complete: ensured={ensured} reused={reused} errors={errors}"
        );
    }
}
