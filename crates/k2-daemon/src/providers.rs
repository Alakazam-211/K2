//! Daemon implementations of the k2so-core awareness provider traits.
//!
//! F1 of Phase 3.1 — `DaemonInjectProvider` + `DaemonWakeProvider` —
//! plus G4 of Phase 3.2, which teaches `DaemonWakeProvider` to
//! ACTUALLY LAUNCH the target agent's session (via the shared
//! `spawn::spawn_agent_session` helper) when an `AGENT.md` launch
//! profile exists for it. Before G4, wake just enqueued to
//! pending-live and waited for a user-triggered spawn; G4 closes
//! that gap so offline agents get auto-spawned on demand.
//!
//! Registered at daemon startup via `register_all()`.

use k2_core::workspace::launch_profile::{load_launch_profile, resolve_cwd, LaunchProfile};
use k2_core::awareness::{AgentAddress, AgentSignal, InjectProvider, WakeProvider};
use k2_core::log_debug;

use crate::session_lookup;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

/// Looks up the target agent's session handle across BOTH the
/// legacy `session_map` (Kessel-T0) and `v2_session_map`
/// (Alacritty_v2) and writes the rendered signal bytes into its
/// PTY. If no session is registered for the target agent in
/// either map, returns `NotFound` — the egress path sees this as
/// "inject failed" and reports it in the `DeliveryReport`; the
/// signal still lands in activity_feed and the bus, so nothing is
/// silently lost.
pub struct DaemonInjectProvider;

impl InjectProvider for DaemonInjectProvider {
    fn inject(&self, agent: &str, bytes: &[u8]) -> std::io::Result<()> {
        let session = session_lookup::lookup_any(agent).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("no live session for agent {agent}"),
            )
        })?;
        // Two-phase write — same pattern as
        // `heartbeat_launch::run_inject`. Write the body, settle for
        // 150ms so the TUI input widget commits each character, then
        // write `\r` as a separate syscall so it's interpreted as
        // Enter rather than the tail of a multi-line paste. A
        // combined `body+\r` single write lands typed-but-not-sent
        // because raw-mode input widgets treat fast-arriving bytes
        // as paste content (claude code, codex, gemini all do this).
        session.write(bytes)?;
        std::thread::sleep(std::time::Duration::from_millis(150));
        session.write(b"\r")
    }

    /// Daemon-side liveness probe. Walks both legacy `session_map`
    /// and `v2_session_map` via `session_lookup`. Without this
    /// override, `egress::is_agent_live` would only see legacy
    /// sessions and route every Live signal targeting a v2-only
    /// agent through the wake provider — bypassing the inject
    /// path entirely.
    fn is_live(&self, agent: &str) -> bool {
        session_lookup::lookup_any(agent).is_some()
    }
}

// NB: the formatter that turns an AgentSignal into bytes-for-PTY
// lives at the crate root (`crate::inject_bytes_for_signal`) —
// both this provider (called by egress::try_inject via k2so-core's
// render function) and the spawn-path drain loop use it. Since
// egress::try_inject formats at its own site and we don't see
// signal context here (only bytes), the provider impl stays
// dead-simple: look up, write.

/// Phase 3.2 G4 wake provider. Two-stage delivery:
///
/// 1. **Durability first:** enqueue the signal to the F3 pending-
///    live queue on disk so it survives daemon restart, OS crash,
///    and spawn failure. Queueing is the unconditional baseline —
///    even if auto-launch fails (no profile, bad spawn config),
///    the signal stays on disk and the next user-triggered spawn
///    will drain it.
///
/// 2. **Auto-launch (G4):** if the target agent has a
///    `<project>/.k2so/agents/<agent>/AGENT.md` launch profile
///    (G3's `launch_profile::load_launch_profile`), spawn a fresh
///    session using that profile. The spawn path then drains the
///    pending-live queue in order, and the just-enqueued signal
///    becomes the session's first byte of input.
///
/// Race-safe single-flight: before spawning, re-check
/// `session_map::lookup(agent)`. If a concurrent path (another
/// signal's wake, a `/cli/sessions/spawn` arriving mid-flight)
/// already registered the session, we skip — the queued signal
/// will be drained by the now-live session path naturally.
///
/// The `from.workspace` field on the signal supplies the project
/// id used to locate `AGENT.md`. After G0's CLI resolver, this
/// is a UUID matching `projects.id`; we look it up to get the
/// filesystem path. Signals with a stale or unknown workspace id
/// fall back to queue-only delivery with a log line.
pub struct DaemonWakeProvider;

impl WakeProvider for DaemonWakeProvider {
    fn wake(&self, agent: &str, signal: &AgentSignal) -> std::io::Result<()> {
        // Stage 1: always enqueue so durability is preserved
        // regardless of what the auto-launch path does next.
        //
        // **0.37.5 keying:** enqueue under the canonical bare
        // `<workspace_id>` key when the signal carries a target
        // workspace. The spawn helper drains under the same canonical
        // key (post-0.37.5 = bare project_id, see
        // `canonical_session::canonical_key_for`), so signals queued
        // while a workspace's agent is offline land in the right
        // session when it boots. Pre-0.37.5 the key was
        // `<workspace_id>:<agent_name>` — that suffix was vestigial
        // post-0.37.0 unification and caused signal drains to miss
        // when the renderer or another caller computed the wrong
        // agent name (C3PO `5c80bef1`).
        let queue_key = match &signal.to {
            AgentAddress::Agent { workspace, .. } if !workspace.0.is_empty() => {
                workspace.0.clone()
            }
            _ => agent.to_string(),
        };
        match crate::pending_live::enqueue(signal, &queue_key) {
            Ok(path) => {
                log_debug!(
                    "[daemon/wake] queued signal id={} for {queue_key} at {:?}",
                    signal.id,
                    path
                );
            }
            Err(e) => {
                log_debug!(
                    "[daemon/wake] failed to queue signal for {queue_key}: {e}"
                );
                return Err(e);
            }
        }

        // Stage 2: G4 auto-launch. Best-effort — any failure here
        // (profile absent, project path unknown, spawn error) leaves
        // the signal queued for a future user-triggered spawn.
        if let Err(reason) = try_auto_launch(agent, signal) {
            log_debug!(
                "[daemon/wake] auto-launch skipped for {agent}: {reason} \
                 (signal stays in queue)"
            );
        }
        Ok(())
    }
}

/// Inner auto-launch path. Lifted out of the trait impl so the
/// "couldn't launch, here's why" branch is explicit and
/// testable.
///
/// Returns `Ok(())` when the spawn succeeded (session_map now has
/// an entry for the agent; the pending queue has been drained).
/// `Err(reason)` covers every skipped-auto-launch path, each with
/// a short reason the caller logs:
///   - agent already has a live session (someone else spawned)
///   - no AGENT.md launch profile on disk
///   - workspace id doesn't map to a known project path
///   - spawn itself failed (bad command, PTY exhaustion, etc.)
fn try_auto_launch(agent: &str, signal: &AgentSignal) -> Result<(), String> {
    // 0.37.0: signal.to.workspace is required and authoritative. The
    // 0.36.15 signal.from fallback is retired — every wake target has
    // a workspace by construction now (CLI, awareness bus, heartbeat).
    let workspace_id = match &signal.to {
        AgentAddress::Agent { workspace, .. } => workspace.0.as_str(),
        AgentAddress::Workspace { workspace } => workspace.0.as_str(),
        _ => return Err("broadcast signal has no target workspace".into()),
    };

    // Single-flight: if a concurrent path already registered THIS
    // workspace's session, skip. The pending-live drain on that
    // session will pick up our enqueued signal.
    //
    // **0.37.5 keying:** look up under the bare `workspace_id`
    // canonical key (see `canonical_session::canonical_key_for`).
    // The spawn helper itself performs the same idempotency check
    // internally; this early lookup is a logged-skip optimization.
    let canonical_key = crate::canonical_session::canonical_key_for(workspace_id);
    if session_lookup::lookup_any(&canonical_key).is_some() {
        return Err("session already live for this workspace".into());
    }

    let project_path = lookup_project_path(workspace_id)
        .ok_or_else(|| format!("no project registered for workspace id {workspace_id:?}"))?;

    // G3 launch profile lookup. Malformed profile = log + skip (bad
    // YAML shouldn't kill the wake path).
    //
    // 0.37.0 ergonomic fallback: AGENT.md without an explicit
    // `launch:` block synthesizes a default `claude
    // --dangerously-skip-permissions` running at the workspace
    // root. This closes the "no launch profile" gap for the common
    // case — workspaces shipped without webhook automation just
    // want their pinned chat agent woken when a wake signal
    // arrives. Custom workflows (Baden's SMS bridge, --print
    // single-shot patterns, custom resume logic) still author an
    // explicit `launch:` block in AGENT.md and override these
    // defaults field-by-field.
    let profile = match load_launch_profile(&project_path, agent) {
        Ok(Some(p)) => p,
        Ok(None) => default_launch_profile(),
        Err(e) => return Err(format!("launch profile parse failed: {e}")),
    };

    // Pass the BARE agent_name + workspace_id; the spawn helper
    // builds the canonical key. Both this auto-launch path and the
    // /cli/agents/launch path now feed the helper the same shape,
    // so they converge on a single slot per (workspace, agent).
    let req = launch_request_for(agent, workspace_id, &project_path, &profile);
    let outcome = spawn_agent_session_v2_blocking(req)
        .map_err(|e| format!("spawn failed: {e}"))?;

    // Write a workspace_sessions DB row so the canonical session is
    // visible across daemon restart and to the Tauri app's
    // re-attach path. Without this, the in-memory v2_session_map
    // is the only source of truth — and Tauri opening AFTER a wake
    // auto-spawn would query workspace_sessions, find nothing, and
    // spawn ITS OWN session under the same canonical key. The
    // canonicalization check in the spawn helper would catch the
    // dup at the in-memory layer, but only after a wasted DaemonPty
    // spawn cycle. Writing the DB row up front lets the Tauri
    // attach path observe the live session before deciding to
    // spawn.
    //
    // Mirrors `agents_routes::handle_agents_launch`'s post-spawn
    // k2so_agents_lock call. Best-effort: failure logs but doesn't
    // unwind the spawn (the PTY is alive regardless).
    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent.to_string(),
        Some(outcome.session_id.to_string()),
        Some("system".to_string()),
    );

    log_debug!(
        "[daemon/wake] auto-launched session={} agent={agent} pending_drained={} \
         via AGENT.md launch profile",
        outcome.session_id,
        outcome.pending_drained,
    );
    Ok(())
}

/// Default launch profile for workspaces whose AGENT.md doesn't
/// declare a `launch:` block. Mirrors the everyday "open the chat
/// tab in Tauri" command — claude in interactive mode with K2SO's
/// permission flag, running at the project root. Custom workflows
/// (heartbeats with --print, automation with --resume, alternate
/// harnesses like codex/gemini, custom cwd) still author an
/// explicit `launch:` block in AGENT.md and override these
/// defaults field-by-field.
fn default_launch_profile() -> LaunchProfile {
    LaunchProfile {
        command: Some("claude".to_string()),
        args: Some(vec!["--dangerously-skip-permissions".to_string()]),
        cwd: None, // None = project root via resolve_cwd
        cols: None,
        rows: None,
        env: Default::default(),
    }
}

/// Lookup `projects.path` by `projects.id`. Returns `None` if the
/// workspace id doesn't match any registered project (signals from
/// an unregistered workspace fall through to queue-only delivery).
fn lookup_project_path(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![workspace_id],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Turn a `LaunchProfile` + project root into a `SpawnWorkspaceSessionRequest`.
/// Applies defaults for every field the profile leaves unset
/// (matching `POST /cli/sessions/spawn`'s defaults for consistency
/// with the explicit-spawn path).
fn launch_request_for(
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

/// Register both providers on the k2so-core ambient singletons.
/// Called once at daemon startup before the accept loop.
pub fn register_all() {
    k2_core::awareness::set_inject_provider(Box::new(DaemonInjectProvider));
    k2_core::awareness::set_wake_provider(Box::new(DaemonWakeProvider));
    log_debug!(
        "[daemon/providers] registered DaemonInjectProvider + DaemonWakeProvider"
    );
}
