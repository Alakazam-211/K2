//! Daemon-owned map of `agent_name → Arc<DaemonPtySession>` for
//! Alacritty_v2 sessions.
//!
//! Parallel to `session_map.rs` (which holds Kessel-T0's
//! `SessionStreamSession`). They're kept separate so v1 / Kessel-T0
//! and v2 can coexist during the transition without sharing a
//! heterogeneous map. Post-cleanup (`.k2so/prds/post-landing-cleanup.md`),
//! this may become the only daemon session map.
//!
//! Lifecycle:
//!   - Inserted by `/cli/sessions/v2/spawn` (added in A4).
//!   - Looked up by `/cli/sessions/grid` WS (added in A3) to find
//!     the session a client is trying to attach to.
//!   - Removed on deliberate tab close (via A6 wiring).
//!
//! `DaemonPtySession` is held inside an `Arc` so the WS handler and
//! the map can each retain a handle independently — dropping the
//! last Arc triggers the IO-thread shutdown naturally.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::DaemonPtySession;

type AgentMap = Arc<Mutex<HashMap<String, Arc<DaemonPtySession>>>>;

static MAP: OnceLock<AgentMap> = OnceLock::new();

fn shared() -> AgentMap {
    MAP.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone()
}

/// Register a live v2 session under `agent_name`.
///
/// 0.37.0 retired the 0.36.14 bare-name mirror: workspace-agent
/// sessions are keyed on `<project_id>:<bare>` exclusively, so the
/// awareness bus and CLI lookups always carry workspace context.
/// Worktree chats and ad-hoc Cmd+T tabs register under their own
/// terminal-id-shaped keys; nothing depends on a bare-name slot.
pub fn register(agent_name: impl Into<String>, session: Arc<DaemonPtySession>) {
    let key = agent_name.into();
    let map_arc = shared();
    let displaced = {
        let mut map = map_arc.lock().unwrap();
        map.insert(key.clone(), Arc::clone(&session))
    };
    // If an existing session was overwritten under this key, its child
    // would otherwise leak: nothing else holds a chokepoint reference
    // to it once it's out of the map. Force-kill + reap it now. `kill()`
    // is idempotent and best-effort, so a session that already exited is
    // a cheap no-op. Done OUTSIDE the map lock (kill() sleeps ~100ms).
    if let Some(old) = displaced {
        if !Arc::ptr_eq(&old, &session) {
            log_debug!(
                "[v2-map] register displaced existing session under key={key}; killing old child"
            );
            old.kill();
        }
    }
    // 0.38.0 Commit 4 — fan out to `/cli/sessions/events` subscribers
    // so connected renderers + the mobile companion learn about new
    // sessions without polling. Best-effort: `let _ =` swallows the
    // "no subscribers" Err that broadcast returns when nothing's
    // listening (the test environments hit this path, as does any
    // pre-WS-attach window during boot).
    let cwd = session
        .cwd
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let pane_group_id_opt = crate::session_events::pane_group_id_from_agent(&key);
    let _ = crate::session_events::emit(
        crate::session_events::SessionEvent::SessionAdded {
            workspace_path: cwd.clone(),
            pane_group_id: pane_group_id_opt.clone(),
            agent_name: key.clone(),
            command: session.program.clone(),
            args: session.args.clone(),
            session_id: session.session_id.to_string(),
            is_v2: true,
        },
    );

    // 0.38.5 — persist session-backing metadata so this PTY's spawn
    // args + session_id survive a daemon restart. Only stamp rows for
    // sessions where we have a resolvable workspace + a canonical
    // pane_group_id; pinned-chat / heartbeat sessions whose
    // agent_name isn't `tab-`-prefixed use the bare agent_name as
    // pane_group_id (the helper handles both shapes). See
    // `0045_workspace_tab_sessions.sql` for the architecture.
    let pane_group_id = pane_group_id_opt.unwrap_or_else(|| key.clone());
    if !cwd.is_empty() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(project_id) = k2_core::workspace::agent_identity::resolve_project_id(&conn, &cwd) {
            // pinned-chat-identity-ssot PRD §4.3.2 (GH#24): the canonical
            // pinned chat (`agent_name == project_id`) must NOT double-book
            // its identity in `workspace_tab_sessions`. Its single source
            // of truth is `workspace_sessions.session_id` (1 row/workspace,
            // `project_id` UNIQUE), stamped by the resolver + the deferred
            // read-back. Writing an argv-derived copy here is the redundant
            // second store that the GH#24 re-mint loop fed on, and now that
            // restart-recovery reads the SSOT (§4.3.1), the tab row is dead
            // weight for the pinned chat. Validated safe to skip: the
            // session-picker (#679) reads chat history FROM DISK
            // (`list_all_sessions` → `parse_claude_sessions`), never this
            // table, so the dropdown is unaffected. Ad-hoc Cmd+T tabs
            // (`agent_name == tab-<paneGroupId>`) still need this row for
            // restart-recovery and are written below as before.
            if key == project_id {
                log_debug!(
                    "[v2-map] register: skipping workspace_tab_sessions stamp for canonical pinned key={key} (identity SSOT is workspace_sessions)"
                );
                return;
            }
            let args_json = serde_json::to_string(&session.args).ok();
            // 0.38.8 — extract claude's session UUID from the args if
            // present. v2_spawn::handle_v2_spawn auto-injects
            // `--session-id <uuid>` for every Cmd+T claude spawn, so
            // we can stamp `workspace_tab_sessions.session_id` here.
            // Restart-recovery in v2_spawn reads this column to splice
            // `--resume <uuid>` on the next daemon restart →
            // conversation continuity for Cmd+T tabs.
            let claude_session_id = session
                .args
                .windows(2)
                .find_map(|w| {
                    if w[0] == "--session-id" || w[0] == "--resume" {
                        Some(w[1].clone())
                    } else {
                        None
                    }
                });
            let row = k2_core::db::schema::WorkspaceTabSession {
                project_id,
                pane_group_id,
                agent_name: key,
                // Set when spawn args carry --session-id / --resume;
                // None otherwise. The upsert uses COALESCE so a
                // subsequent re-register without the flag won't
                // clobber a previously-stamped value.
                session_id: claude_session_id,
                command: session.program.clone(),
                args_json,
                cwd: Some(cwd),
                last_seen_at: 0, // ignored — table default is unixepoch()
            };
            let _ = k2_core::db::schema::WorkspaceTabSession::upsert(&conn, &row);
        }
    }
}

/// Remove the map entry. Returns the Arc if one was present;
/// subsequent drops of all holders tear the session down.
///
/// Runs the active-session cleanup path: any `agent_heartbeats` or
/// `workspace_sessions` row whose `active_terminal_id` matches the
/// removed session's id is nulled, and the matching workspace's row
/// gets `surfaced=0` + `status='sleeping'`. This is the single
/// chokepoint for "v2 session goes away" — child-exit observer in
/// v2_spawn invokes us, the explicit /v2/close route invokes us, the
/// watchdog escalation path invokes us. See the
/// `heartbeat-active-session-tracking` PRD.
///
/// 0.37.0: with `workspace_sessions` keyed on `project_id` and the
/// `agent_name` column gone, the cleanup is keyed entirely on the
/// terminal_id we just stopped. The pre-0.37.0 dual-cleanup logic
/// (prefix split → scoped UPDATE by `(project_id, agent_name)`) is
/// retired.
pub fn unregister(agent_name: &str) -> Option<Arc<DaemonPtySession>> {
    let map_arc = shared();
    let removed = {
        let mut map = map_arc.lock().unwrap();
        map.remove(agent_name)
    };

    if let Some(ref session) = removed {
        // 0.38.0 Commit 4 — push to `/cli/sessions/events` subscribers
        // BEFORE the DB cleanup so the renderer sees the drop event
        // alongside (or just before) the existing surfaced/sleeping
        // flips. Best-effort: emit returns Err when no subscribers
        // are attached; callers don't care.
        let cwd_emit = session
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = crate::session_events::emit(
            crate::session_events::SessionEvent::SessionRemoved {
                workspace_path: cwd_emit.clone(),
                pane_group_id: crate::session_events::pane_group_id_from_agent(agent_name),
                agent_name: agent_name.to_string(),
            },
        );

        let terminal_id = session.session_id.to_string();
        let db = k2_core::db::shared();
        let conn = db.lock();
        // 0.39.39 (#677.1) — a heartbeat session's live state flips to
        // false when its PTY exits. Resolve which heartbeat(s) pointed at
        // this terminal BEFORE we null the column, then broadcast
        // `HeartbeatStateChanged{live:false}` so every client converges
        // the live-dot without polling. Best-effort: the broadcast
        // `let _ =`-swallows the no-subscribers case.
        if let Ok(hbs) =
            k2_core::db::schema::AgentHeartbeat::find_by_active_terminal(&conn, &terminal_id)
        {
            for (project_id, name) in hbs {
                let _ = crate::session_events::emit(
                    crate::session_events::SessionEvent::HeartbeatStateChanged {
                        workspace_path: cwd_emit.clone(),
                        project: project_id,
                        agent: name,
                        live: false,
                    },
                );
            }
        }
        let _ = k2_core::db::schema::AgentHeartbeat::clear_active_terminal_id_by_terminal(
            &conn,
            &terminal_id,
        );
        // Mirror of the heartbeat cleanup above (migration 0037): the
        // chat tab's pinned workspace_sessions row stamps its own
        // active_terminal_id on v2 spawn. PTY exit nulls it here so
        // the next mount's `/cli/sessions/lookup-by-agent` sees the
        // truth.
        let _ = k2_core::db::schema::WorkspaceSession::clear_active_terminal_id_by_terminal(
            &conn,
            &terminal_id,
        );
        // Flip surfaced=0 + status=sleeping for the workspace whose
        // active_terminal_id matched. Targeting by terminal_id (rather
        // than (project_id, agent_name)) means this single UPDATE
        // covers every code path — chat tab, heartbeat headless wake,
        // worktree chat — without needing to know which kind of
        // session this was.
        let _ = conn.execute(
            "UPDATE workspace_sessions SET surfaced = 0, status = 'sleeping' \
             WHERE terminal_id = ?1 OR active_terminal_id = ?1",
            rusqlite::params![terminal_id],
        );
        drop(conn);

        // Force-kill + reap the child. This is the single "v2 session
        // goes away" chokepoint (deliberate close, child-exit observer,
        // watchdog escalation all route here), so killing here is what
        // stops agent-CLI children orphaning. Pre-fix we relied on the
        // Arc drop → channel close → single SIGHUP, which agent CLIs
        // ignore/outlive (the multi-GB leak). `kill()` is idempotent;
        // if the child already exited it's a no-op. Note we still hold
        // `removed` (the returned Arc) so the session object outlives
        // this call — Drop will call kill() again later as a no-op.
        session.kill();
    }
    removed
}

/// Lookup by agent name. Called on find-or-spawn to decide
/// whether to reuse an existing session.
pub fn lookup_by_agent_name(agent_name: &str) -> Option<Arc<DaemonPtySession>> {
    shared().lock().unwrap().get(agent_name).cloned()
}

/// Lookup by `SessionId`. Iterates the map — O(N) where N is the
/// number of live v2 sessions. Called on every WS grid attach to
/// resolve the requested session. N is expected to stay small
/// (a handful of open Tauri tabs at most).
pub fn lookup_by_session_id(id: &SessionId) -> Option<Arc<DaemonPtySession>> {
    shared()
        .lock()
        .unwrap()
        .values()
        .find(|s| s.session_id == *id)
        .cloned()
}

/// Every registered (agent_name, session) pair. Returning owned
/// Arcs lets the caller drop the map lock before doing expensive
/// work against the sessions. Ordering is unspecified.
pub fn snapshot() -> Vec<(String, Arc<DaemonPtySession>)> {
    shared()
        .lock()
        .unwrap()
        .iter()
        .map(|(name, session)| (name.clone(), Arc::clone(session)))
        .collect()
}

/// All registered agent names. Used by diagnostic endpoints.
#[allow(dead_code)]
pub fn list_agents() -> Vec<String> {
    shared().lock().unwrap().keys().cloned().collect()
}

/// Test helper — drop every registered entry. Keeps tests that
/// share the global map from contaminating each other.
/// Drop every registered entry. Available to both unit tests
/// (in this module) and integration tests (in `tests/*.rs`) so
/// shared global state doesn't leak between cases.
pub fn clear_for_tests() {
    shared().lock().unwrap().clear();
}

/// 0.37.5 boot-time migration — re-key any entry whose key shape is
/// `<uuid>:<rest>` to the bare `<uuid>` form (the new canonical
/// shape). Pre-0.37.5 the canonical key encoded the agent name as a
/// suffix; post-0.37.5 it's bare project_id (see
/// `canonical_session::canonical_key_for`).
///
/// **Defensive on a fresh daemon boot.** The map is empty at boot,
/// so this helper is a no-op in the common case. It earns its
/// keep when the daemon stays running across a binary upgrade
/// (upgrade-without-restart): old entries linger under the legacy
/// shape, the renderer post-upgrade asks under the new shape and
/// misses, fresh PTY spawns. This sweep collapses the old entries
/// into the new shape so lookups land. Idempotent.
///
/// **Atomicity.** Holds the map lock for the entire snapshot+rekey
/// pass so a concurrent register/unregister can't see a half-migrated
/// state. Per-entry collision (both old + new shapes registered at
/// the same time) keeps the bare-keyed one and drops the legacy.
pub fn migrate_legacy_keys_to_bare_pid() {
    let map_arc = shared();
    let mut map = map_arc.lock().unwrap();
    let mut migrated = 0usize;
    let mut collided = 0usize;
    let legacy_keys: Vec<String> = map
        .keys()
        .filter(|k| is_legacy_canonical_key(k))
        .cloned()
        .collect();
    for legacy in legacy_keys {
        let prefix = match legacy.split_once(':') {
            Some((p, _)) => p.to_string(),
            None => continue,
        };
        let arc = match map.remove(&legacy) {
            Some(a) => a,
            None => continue,
        };
        if map.contains_key(&prefix) {
            // Both shapes present — keep the bare-keyed (already
            // canonical) entry, drop the legacy. The dropped Arc's
            // ChildExit observer will fire on the orphaned PTY's
            // child exit; v2_session_map::unregister no-ops if the
            // key isn't present.
            collided += 1;
            log_debug!(
                "[v2-map/migrate] both shapes present for {prefix}; dropping legacy {legacy}"
            );
            continue;
        }
        map.insert(prefix.clone(), arc);
        migrated += 1;
        log_debug!("[v2-map/migrate] re-keyed {legacy} → {prefix}");
    }
    if migrated > 0 || collided > 0 {
        log_debug!(
            "[v2-map/migrate] complete: migrated={migrated} collided={collided}"
        );
    }
}

fn is_legacy_canonical_key(k: &str) -> bool {
    // UUID-shaped prefix (36 chars + colon-then-suffix) signals
    // pre-0.37.5 canonical key. Tab keys (`tab-XXX`), worktree
    // (no colon), and bare-pid keys (no colon) all fail this check.
    if k.len() < 38 || !k.is_char_boundary(36) {
        return false;
    }
    let bytes = k.as_bytes();
    bytes[8] == b'-'
        && bytes[13] == b'-'
        && bytes[18] == b'-'
        && bytes[23] == b'-'
        && bytes[36] == b':'
}
