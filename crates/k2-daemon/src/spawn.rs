//! Shared session-spawn helper used by every code path that launches
//! a daemon-owned PTY session.
//!
//! In 0.39.0 the legacy v1 (`SessionStreamSession` / Kessel-T0) path
//! was retired along with `/cli/sessions/spawn`. What remains is the
//! Alacritty_v2 helper (`spawn_agent_session_v2_blocking`) which is
//! called from `/cli/sessions/v2/spawn`, `DaemonWakeProvider::wake`,
//! and other daemon-internal launchers.

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};

use crate::pending_live;
use crate::signal_format;
use crate::v2_session_map;

/// Resolve a workspace's filesystem path from its project_id.
/// Used by the v2 spawn helper to seed the session label from the
/// agent display name (Phase B). `None` if the row is gone.
fn project_path_for_id(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

/// Input shape for a spawn. Shared by the HTTP handler
/// (`/cli/sessions/v2/spawn`) and the scheduler-wake path
/// (`DaemonWakeProvider`).
///
/// 0.37.0 canonicalization: every spawn carries `project_id` so the
/// v2 spawn helper can register under the canonical bare-`<project_id>`
/// key. Pre-0.37.0, callers passed bare `agent_name` and registered
/// under the bare key — `agents launch` and `--wake`'s auto-launch
/// path then ended up writing to different slots in the same map
/// (bare vs prefixed) and accumulating duplicate sessions per
/// workspace. With `project_id` set, both paths converge on the same
/// key and the canonical-session invariant holds.
///
/// `project_id` remains `Option<String>` for the rare bare-name
/// caller (no workspace context available). v2 callers that DO have
/// workspace context should always set it.
#[derive(Debug, Clone, Default)]
pub struct SpawnWorkspaceSessionRequest {
    pub agent_name: String,
    pub project_id: Option<String>,
    pub cwd: String,
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub cols: u16,
    pub rows: u16,
    /// 0.37.8 — explicit canonical_key override. When `Some`, the v2
    /// spawn registers under THIS key in `v2_session_map` instead of
    /// the default `canonical_key_for(project_id)`. Used by heartbeat
    /// fires (`<project_id>:hb:<heartbeat_name>`) so they don't
    /// collide with the workspace's pinned chat session at the bare
    /// `<project_id>` slot. Default `None` = use the project_id-derived
    /// canonical key (the chat-tab lane).
    pub canonical_key: Option<String>,
}

/// Output shape returned by the spawn helper. The caller needs the
/// session id (unique across the daemon), the agent name it ended up
/// registered as, and how many pending-live signals were drained on
/// boot. The owning `Arc` lives in `v2_session_map`.
///
/// `reused = true` when the v2 spawn helper found an existing
/// session under the canonical `<project_id>` key and returned it
/// instead of spawning a duplicate (idempotent `agents launch`).
/// When `reused` is true, `pending_drained` is 0 (the session was
/// already alive; any pending-live signals were drained on its
/// original spawn).
#[derive(Clone, Debug)]
pub struct SpawnWorkspaceSessionOutcome {
    pub session_id: SessionId,
    pub agent_name: String,
    pub pending_drained: usize,
    pub reused: bool,
}

/// Alacritty_v2 spawn helper. Takes a `SpawnWorkspaceSessionRequest`
/// and returns a `SpawnWorkspaceSessionOutcome`; the produced session
/// is a `DaemonPtySession` registered in `v2_session_map`.
///
/// 0.37.0 canonicalization: when `req.project_id` is set, the session
/// registers under the prefixed key `<project_id>:<agent_name>` and
/// the function performs an **idempotency check** first — if a session
/// is already registered under that key and its child PID is still
/// alive, return the existing session_id with `reused=true` instead
/// of spawning a duplicate. This is what makes `agents launch`
/// idempotent end-to-end and closes Baden's "session A vs B
/// accumulating" bug class.
///
/// When `req.project_id` is None (legacy bare-name callers, vanishingly
/// rare post-0.37.0), registration falls back to the bare agent_name
/// — the function works but the canonicalization invariant doesn't
/// apply. Production daemon callers always set project_id.
///
/// Synchronous: alacritty's IO thread is started in the background
/// by `DaemonPtySession::spawn`; this fn returns as soon as the PTY
/// is open and the Term + event loop are wired. No grow-then-shrink
/// dance is needed (v2 doesn't have the replay-ring seeding the
/// legacy path requires).
pub fn spawn_agent_session_v2_blocking(
    req: SpawnWorkspaceSessionRequest,
) -> Result<SpawnWorkspaceSessionOutcome, String> {
    if req.agent_name.is_empty() {
        return Err("agent_name required".into());
    }

    // Canonical key construction.
    //
    // **0.37.5 contract:** every workspace has exactly one canonical
    // slot in v2_session_map, keyed on `project_id` alone. The agent's
    // name is metadata about the session (display label, persona file,
    // launch profile owner) — not part of the address. Pre-0.37.5 the
    // key was `<project_id>:<agent_name>`; the suffix was vestigial
    // post-0.37.0 unification (one agent per workspace) and caused
    // the renderer to compute the wrong key when its mode→name
    // mapping disagreed with AGENT.md's `name:` field. See
    // `canonical_session::canonical_key_for` for the full rationale.
    //
    // Project_id-less callers (delegated worktrees via
    // `/cli/agents/delegate`, legacy bare-name spawns) still register
    // under the bare agent_name — those don't represent canonical
    // workspace sessions and don't go through this code's
    // workspace-aware paths.
    // 0.37.8 — explicit canonical_key override takes precedence so
    // heartbeat fires can stay in their own per-heartbeat lane
    // (`<project_id>:hb:<name>`) without colliding with the chat tab's
    // bare-`<project_id>` slot. Default falls through to the
    // project_id-derived canonical key.
    let canonical_key = match req.canonical_key.as_deref() {
        Some(k) if !k.is_empty() => k.to_string(),
        _ => match req.project_id.as_deref() {
            Some(pid) if !pid.is_empty() => crate::canonical_session::canonical_key_for(pid),
            _ => req.agent_name.clone(),
        },
    };

    // Idempotency: if a session is already registered under the
    // canonical key AND its child PID is alive, return it. The map
    // is normally kept authoritative by the child-exit observer
    // (`v2_spawn::spawn_child_exit_observer`), but the `is_child_alive`
    // double-check closes the small race window between ChildExit
    // and unregister — and gracefully handles the rare case where
    // the observer task panicked or the broadcast channel closed
    // before ChildExit landed.
    if let Some(existing) = v2_session_map::lookup_by_agent_name(&canonical_key) {
        if existing.is_child_alive() {
            log_debug!(
                "[daemon/spawn] v2 reuse session={} canonical_key={}",
                existing.session_id,
                canonical_key,
            );
            return Ok(SpawnWorkspaceSessionOutcome {
                session_id: existing.session_id,
                agent_name: req.agent_name,
                pending_drained: 0,
                reused: true,
            });
        }
        // Stale entry — child has exited but the unregister hadn't
        // fired yet (or the observer dropped its Arc). Clean up and
        // continue to spawn fresh.
        log_debug!(
            "[daemon/spawn] reaping stale v2 entry under canonical_key={} (child exited)",
            canonical_key,
        );
        v2_session_map::unregister(&canonical_key);
    }

    // Phase B: seed the session's label with the workspace's
    // primary-agent display name and LOCK it. This is the
    // canonical workspace+agent session — its tab label should be
    // the friendly agent name (per the agent-display-name PRD),
    // never `claude --resume`'s "Claude Code" PTY title. Locked
    // means PTY title events still drive activity-marker
    // detection but cannot mutate the visible label.
    //
    // Resolving the label needs a project_path; we have project_id.
    // Look the path up via the agents-display helper's caching
    // resolver — falls back to the agent name when no project
    // row exists (legacy bare-name path) so the label still has
    // SOMETHING readable.
    let label_seed = if let Some(pid) = req.project_id.as_deref() {
        if !pid.is_empty() {
            project_path_for_id(pid)
                .map(|p| k2_core::workspace::display::agent_display_name(&p))
                .unwrap_or_else(|| req.agent_name.clone())
        } else {
            req.agent_name.clone()
        }
    } else {
        req.agent_name.clone()
    };

    // Convert the request shape into DaemonPtyConfig. v2 takes its
    // working directory as `Option<PathBuf>` rather than a String,
    // and stores `program: Option<String>` (vs legacy `command`),
    // but the wire shape is otherwise identical.
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: req.cols,
        rows: req.rows,
        cwd: Some(PathBuf::from(&req.cwd)),
        program: req.command.clone(),
        args: req.args.clone().unwrap_or_default(),
        env: HashMap::new(),
        drain_on_exit: true,
        label: label_seed,
        label_source: k2_core::terminal::LabelSource::Locked,
    };
    let session_id = cfg.session_id;

    let session = DaemonPtySession::spawn(cfg)
        .map_err(|e| format!("v2 spawn failed: {e}"))?;
    v2_session_map::register(canonical_key.clone(), session.clone());

    // Wire the child-exit observer so the v2_session_map slot
    // releases when the child PID dies — keeps the idempotency
    // check above accurate without a separate liveness probe.
    crate::v2_spawn::spawn_child_exit_observer(canonical_key.clone(), session.clone());

    // Drain any pending-live signals queued for this canonical key
    // so they become input to the fresh session. Awareness bus
    // enqueues under the prefixed key (post-0.36.15); this drain
    // matches that. Without this, signals enqueued by
    // `DaemonWakeProvider::wake` while the agent was offline get
    // silently dropped on v2 boot.
    let pending = pending_live::drain_for_agent(&canonical_key);
    let pending_drained = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        session.write(bytes.into_bytes());
    }

    log_debug!(
        "[daemon/spawn] v2 session={} canonical_key={} pending_drained={}",
        session_id,
        canonical_key,
        pending_drained,
    );

    Ok(SpawnWorkspaceSessionOutcome {
        session_id,
        agent_name: req.agent_name,
        pending_drained,
        reused: false,
    })
}

