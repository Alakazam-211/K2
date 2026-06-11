//! HTTP handlers for Alacritty_v2 session spawn + close.
//!
//! Endpoints (registered in main.rs):
//!   - `POST /cli/sessions/v2/spawn` — find-or-spawn by agent_name.
//!   - `POST /cli/sessions/v2/close` — explicit session teardown.
//!
//! Parallel to `awareness_ws::handle_sessions_spawn` /
//! `handle_sessions_close` which handle v1 / Kessel-T0's
//! `SessionStreamSession`. Kept separate so the two renderer paths
//! don't step on each other; v1's handlers stay untouched during
//! the v2 transition.
//!
//! Find-or-spawn semantics: the client (Tauri) calls this on every
//! tab mount. If a session already exists for the requested
//! `agent_name` (`tab-<terminalId>`), we return its existing
//! `{sessionId, cols, rows}` with `reused: true`. Otherwise we
//! spawn a fresh `DaemonPtySession` and register it. Tauri always
//! calls the same endpoint whether it's a cold launch or a reattach
//! after workspace swap / app quit. See `.k2so/prds/alacritty-v2.md`
//! phase A4.

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession};

use crate::awareness_ws::HandlerResult;
use crate::pending_live;
use crate::signal_format;
use crate::v2_session_map;

/// Cheap UUID-shape probe: 36 chars, hyphens at the canonical
/// positions (8-4-4-4-12). Used to distinguish a bare `project_id`
/// canonical key (post-0.37.5) from legacy `<pid>:<agent>` strings
/// or ad-hoc tab keys (`tab-XXX`). Doesn't validate the hex digits
/// — fast path for the spawn helper's stamping logic.
fn is_uuid_shape(s: &str) -> bool {
    s.len() == 36
        && s.as_bytes()[8] == b'-'
        && s.as_bytes()[13] == b'-'
        && s.as_bytes()[18] == b'-'
        && s.as_bytes()[23] == b'-'
}

/// Handler for `POST /cli/sessions/v2/spawn`.
///
/// Request body (JSON):
/// ```json
/// {
///   "agent_name": "tab-<terminalId>",
///   "cwd": "/optional/path",
///   "command": "optional program",
///   "args": ["optional", "args"],
///   "cols": 120,
///   "rows": 40,
///   "env": { "KEY": "val" }
/// }
/// ```
///
/// Response body (JSON):
/// ```json
/// {
///   "sessionId": "<uuid>",
///   "agentName": "tab-<terminalId>",
///   "cols": 120,
///   "rows": 40,
///   "reused": false
/// }
/// ```
///
/// `reused: true` means the caller's `agent_name` already had a
/// live session; we returned its handle instead of spawning.
/// Tauri's attach path treats reused and fresh identically.
pub fn handle_v2_spawn(body: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct SpawnRequest {
        agent_name: String,
        #[serde(default = "default_cwd")]
        cwd: String,
        #[serde(default)]
        command: Option<String>,
        #[serde(default)]
        args: Option<Vec<String>>,
        #[serde(default = "default_cols")]
        cols: u16,
        #[serde(default = "default_rows")]
        rows: u16,
        #[serde(default)]
        env: Option<HashMap<String, String>>,
        /// Phase B: caller-supplied initial label. Sent to the
        /// first WS subscriber as `LabelInitial`. Empty/absent ⇒
        /// no seed; `Pty` source means PTY title events fill it.
        #[serde(default)]
        label: Option<String>,
        /// Phase B: lock the label so future PTY title events
        /// (e.g. claude --resume emitting "Claude Code") cannot
        /// overwrite it. Common cases: canonical workspace+agent
        /// session, heartbeat fire sessions, restored chat-history
        /// tabs whose label is the session-derived friendly name.
        #[serde(default)]
        label_locked: Option<bool>,
    }
    fn default_cwd() -> String {
        std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
    }
    fn default_cols() -> u16 {
        80
    }
    fn default_rows() -> u16 {
        24
    }

    let __t_total = std::time::Instant::now();

    let req: SpawnRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse v2 SpawnRequest: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    if req.agent_name.is_empty() {
        return HandlerResult {
            status: "400 Bad Request",
            body: r#"{"error":"agent_name required"}"#.into(),
        };
    }

    // Find-or-spawn: existing session wins. The response preserves
    // whatever cols/rows the existing session was opened at — the
    // caller will ResizeObserver-correct if its viewport differs.
    let __t_lookup = std::time::Instant::now();
    let existing = v2_session_map::lookup_by_agent_name(&req.agent_name);
    let lookup_ms = __t_lookup.elapsed().as_secs_f64() * 1000.0;

    // A stale map entry whose child already exited must NOT be reused —
    // doing so would hand the caller a dead PTY. Evict + reap it (the
    // reap is belt-and-suspenders: a child that exited on its own is
    // usually already gone, but a half-dead process group or an
    // unobserved exit could leave a straggler) and fall through to spawn
    // a fresh replacement below.
    if let Some(existing) = existing.as_ref() {
        if !existing.is_child_alive() {
            log_debug!(
                "[v2-spawn] existing session for agent={} has a dead child; evicting + killing before respawn",
                req.agent_name
            );
            // unregister runs the DB/active-session cleanup chokepoint
            // and itself calls kill(); the explicit kill() here is
            // idempotent and guarantees teardown even if the key was
            // already removed by a racing unregister.
            v2_session_map::unregister(&req.agent_name);
            existing.kill();
        }
    }
    let existing = existing.filter(|s| s.is_child_alive());

    if let Some(existing) = existing {
        let (cols, rows) = current_dims(&existing);
        let session_id_str = existing.session_id.to_string();
        let total_ms = __t_total.elapsed().as_secs_f64() * 1000.0;
        log_debug!(
            "[v2-perf] side=daemon SPAWN_SUMMARY session={} agent={} reused=true total_ms={:.3} lookup_ms={:.3} dpty_spawn_ms=0",
            session_id_str, req.agent_name, total_ms, lookup_ms
        );
        let out = serde_json::json!({
            "sessionId": session_id_str,
            "agentName": req.agent_name,
            "cols": cols,
            "rows": rows,
            "reused": true,
        });
        return HandlerResult {
            status: "200 OK",
            body: out.to_string(),
        };
    }

    // 0.38.5 — restart-recovery: if the daemon was just restarted (app
    // update / launchctl kickstart / crash) the in-memory
    // v2_session_map is empty but `workspace_tab_sessions` still has
    // the prior spawn's command + args + (claude) session_id. When
    // the renderer's spawn request lands with empty command (which
    // v2 schema makes the default for terminal items), substitute
    // the persisted values so we re-run e.g. `claude --resume <id>`
    // instead of dropping the user into a bare shell. Renderer-side
    // command takes precedence on first spawn; only the empty case
    // consults the table. See `0045_workspace_tab_sessions.sql`.
    let mut command = req.command.clone();
    let mut args = req.args.clone().unwrap_or_default();
    if command.is_none() {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(project_id) = k2_core::workspace::agent_identity::resolve_project_id(&conn, &req.cwd) {
            // pinned-chat-identity-ssot PRD §4.3.1 (GH#24): the canonical
            // pinned chat (`agent_name == project_id`) sources its
            // `--resume` id from the SINGLE SOURCE OF TRUTH —
            // `workspace_sessions.session_id` — NOT from
            // `workspace_tab_sessions`. Recovery used to read the tab
            // table's argv-derived id, which is the coupling that let the
            // daemon-owned pinned chat (#683) bypass the canonical column
            // for identity. Command/cwd may still come from the tab row if
            // one happens to exist (legacy/pre-Phase-3), but after Phase 3
            // the pinned row is no longer written, so we default to
            // `claude` + the request cwd and splice `--resume <ssot-id>`.
            // Ad-hoc Cmd+T tabs (`agent_name == tab-<...>`) are untouched
            // below — they have no workspace_sessions row and legitimately
            // recover from workspace_tab_sessions.
            let is_pinned = req.agent_name == project_id;

            // Default the command from the tab row when present; the
            // pinned chat falls back to `claude` so recovery works even
            // with no tab row (Phase 3).
            let tab_row = k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(
                &conn,
                &project_id,
                &req.agent_name,
            )
            .ok()
            .flatten();

            // Source the resume id: SSOT for the pinned chat, the tab
            // row's argv-derived id for ad-hoc tabs.
            let resume_id: Option<String> = if is_pinned {
                k2_core::db::schema::WorkspaceSession::get(&conn, &project_id)
                    .ok()
                    .flatten()
                    .and_then(|row| row.session_id)
                    .filter(|s| !s.is_empty())
            } else {
                tab_row.as_ref().and_then(|r| r.session_id.clone())
            };

            // Pick the command + base args. Tab row wins when it carries a
            // command; the pinned chat defaults to `claude` so it recovers
            // even after Phase 3 drops the tab-row write.
            let recovered_cmd = tab_row
                .as_ref()
                .and_then(|r| r.command.clone())
                .or_else(|| if is_pinned { Some("claude".to_string()) } else { None });

            if let Some(saved_cmd) = recovered_cmd {
                let mut saved_args: Vec<String> = tab_row
                    .as_ref()
                    .and_then(|r| r.args_json.as_deref())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_else(|| {
                        // No tab row (pinned, Phase 3): start from the
                        // standard pinned-chat base flags so a recovered
                        // claude still runs headlessly.
                        vec!["--dangerously-skip-permissions".to_string()]
                    });
                // If we have a resume id, strip any existing
                // `--session-id` flag (we replace it with `--resume` for
                // unambiguous resumption) and splice in `--resume <id>`.
                // The base args carry --dangerously-skip-permissions and
                // similar flags we want to keep.
                if let Some(sid) = resume_id.as_deref() {
                    // Drop any --session-id <value> pair.
                    let mut i = 0;
                    while i + 1 < saved_args.len() {
                        if saved_args[i] == "--session-id" {
                            saved_args.remove(i); // flag
                            saved_args.remove(i); // value
                        } else {
                            i += 1;
                        }
                    }
                    let already_has_resume = saved_args
                        .iter()
                        .any(|a| a == "--resume" || a == "-r");
                    if !already_has_resume {
                        saved_args.push("--resume".to_string());
                        saved_args.push(sid.to_string());
                    }
                }
                log_debug!(
                    "[v2-spawn] restart-recovery: project={} agent={} pinned={} resume_source={} replayed command={} args={:?}",
                    project_id,
                    req.agent_name,
                    is_pinned,
                    if is_pinned { "workspace_sessions(SSOT)" } else { "workspace_tab_sessions" },
                    saved_cmd,
                    saved_args
                );
                command = Some(saved_cmd);
                args = saved_args;
            }
        }
    }

    // 0.38.8 — Cmd+T session continuity. When spawning `claude` with
    // no `--session-id` and no `--resume` (the common Cmd+T-from-the-
    // Tauri-renderer shape), mint a fresh UUID and inject
    // `--session-id <uuid>` so claude persists its conversation to a
    // known-id JSONL. The v2_session_map::register hook reads the
    // injected flag and stamps `workspace_tab_sessions.session_id`,
    // which makes the restart-recovery branch above splice
    // `--resume <uuid>` on the next daemon restart. Net: Cmd+T tabs
    // resume the same conversation after app updates / kickstart.
    if command.as_deref() == Some("claude") {
        let has_session_id = args.iter().any(|a| a == "--session-id");
        let has_resume = args.iter().any(|a| a == "--resume" || a == "-r");
        if !has_session_id && !has_resume {
            let new_sid = uuid::Uuid::new_v4().to_string();
            log_debug!(
                "[v2-spawn] auto-injected --session-id={} for agent={} cwd={}",
                new_sid, req.agent_name, req.cwd
            );
            args.push("--session-id".to_string());
            args.push(new_sid);
        }
    }

    // Spawn a fresh session.
    // Phase B: pick the label seed + source. Caller-supplied label
    // (with optional lock) takes priority; otherwise we leave the
    // label empty and let PTY title events fill it (the legacy
    // pre-Phase-B behavior). `label_locked: true` with no label
    // still locks (caller is saying "don't accept ANY PTY title").
    let seed_label = req.label.clone().unwrap_or_default();
    let label_locked_flag = req.label_locked.unwrap_or(false);
    let label_source = if label_locked_flag {
        k2_core::terminal::LabelSource::Locked
    } else if !seed_label.is_empty() {
        k2_core::terminal::LabelSource::Seed
    } else {
        k2_core::terminal::LabelSource::Pty
    };
    let cfg = DaemonPtyConfig {
        session_id: SessionId::new(),
        cols: req.cols,
        rows: req.rows,
        cwd: Some(PathBuf::from(&req.cwd)),
        program: command,
        args,
        env: req.env.unwrap_or_default(),
        drain_on_exit: true,
        label: seed_label,
        label_source,
    };
    let session_id_for_response = cfg.session_id;

    let __t_spawn = std::time::Instant::now();
    let session = match DaemonPtySession::spawn(cfg) {
        Ok(s) => s,
        Err(e) => {
            return HandlerResult {
                status: "500 Internal Server Error",
                body: format!(
                    r#"{{"error":"v2 spawn failed: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };
    let dpty_spawn_ms = __t_spawn.elapsed().as_secs_f64() * 1000.0;

    v2_session_map::register(req.agent_name.clone(), session.clone());

    // Stamp the agent_sessions row's `active_terminal_id` (migration
    // 0037). Best-effort: tab-keyed spawns (`tab-<id>`) won't have
    // a matching workspace_sessions row and the UPDATE no-ops;
    // workspace agent spawns do, and the column lets the next chat
    // tab mount re-attach without walking the in-memory
    // v2_session_map. Mirror of the heartbeat smart-launch stamp
    // (`heartbeat_launch.rs`) and the
    // `agent_heartbeats.active_terminal_id` cleanup hook in
    // `v2_session_map::unregister`.
    //
    // **0.37.5 keying:** the canonical `req.agent_name` is now bare
    // `<project_id>` (UUID). We accept three shapes for back-compat
    // during the cross-version transition window:
    //   1. Bare UUID: the project_id directly. (post-0.37.5 native)
    //   2. `<project_id>:<bare>`: legacy renderer form. Split, take
    //      the prefix.
    //   3. Anything else (tab-XXX, ad-hoc): resolve from cwd.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let project_id_opt = if is_uuid_shape(&req.agent_name) {
            // Native bare-pid canonical key.
            Some(req.agent_name.clone())
        } else if let Some((pid, _bare)) = req.agent_name.split_once(':') {
            // Legacy `<pid>:<agent>` shape from a 0.37.4 renderer.
            if pid.is_empty() {
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &req.cwd)
            } else {
                Some(pid.to_string())
            }
        } else {
            k2_core::workspace::agent_identity::resolve_project_id(&conn, &req.cwd)
        };
        if let Some(project_id) = project_id_opt {
            // Symmetric session-id-keyed stamping — mirrors the
            // heartbeat update below. The pinned tab and any
            // heartbeats may share the same `<workspace>:<agent>`
            // claude session; whoever resumes that session gets
            // their `active_terminal_id` updated to the new PTY.
            // Walking `req.args` catches both `--resume <id>` and
            // `--session-id <id>`. Without keying on the actual
            // resume target, ad-hoc tab spawns (Cmd+T, AI editor)
            // would clobber the pinned tab's stamp on every spawn.
            let args = &session.args;
            let mut resumed_session: Option<&str> = None;
            let mut i = 0;
            while i + 1 < args.len() {
                if (args[i] == "--resume" || args[i] == "--session-id")
                    && !args[i + 1].is_empty()
                {
                    resumed_session = Some(args[i + 1].as_str());
                    break;
                }
                i += 1;
            }
            if let Some(claude_sid) = resumed_session {
                let new_tid = session.session_id.to_string();

                // Pinned-tab pointer: workspace_sessions row whose
                // saved claude session_id matches what this PTY is
                // resuming. Tab spawns that aren't resuming the
                // pinned tab's session no-op.
                let _ = conn.execute(
                    "UPDATE workspace_sessions SET active_terminal_id = ?1 \
                     WHERE project_id = ?2 AND session_id = ?3",
                    rusqlite::params![&new_tid, &project_id, claude_sid],
                );

                // Heartbeat pointer: any heartbeat in this workspace
                // whose `last_session_id` matches what this PTY is
                // resuming. Multiple heartbeats can target the same
                // claude session (and the workspace's pinned chat),
                // so they all get stamped together.
                let _ = conn.execute(
                    "UPDATE workspace_heartbeats SET active_terminal_id = ?1 \
                     WHERE project_id = ?2 AND last_session_id = ?3",
                    rusqlite::params![&new_tid, &project_id, claude_sid],
                );
            }
        }
    }

    // Child-exit observer: subscribe to the session's alacritty event
    // broadcast and call v2_session_map::unregister when ChildExit
    // arrives. The unregister hook (in v2_session_map) is what nulls
    // any matching agent_heartbeats.active_terminal_id and flips
    // surfaced=0 on the agent_sessions row. Without this, claude
    // --print sessions exit cleanly and leave the column pointing at
    // a corpse — which the lazy cleanup on read would catch
    // eventually, but eventually-consistent stale data is the kind
    // of "feels haunted" UX we'd rather avoid. See
    // `heartbeat-active-session-tracking` PRD.
    spawn_child_exit_observer(req.agent_name.clone(), session.clone());

    // Drain any pending-live signals that were queued while this
    // agent was offline so they become input to the fresh session.
    // Mirrors `crate::spawn::spawn_agent_session`'s legacy drain so
    // wake-queued signals to v2 agents aren't silently lost on boot.
    //
    // Two-phase write per signal — body, settle, `\r` — same pattern
    // `DaemonInjectProvider::inject` and `heartbeat_launch::run_inject`
    // use. A single combined write would be treated as a multi-line
    // paste by the TUI input widget and the queued message would land
    // typed-but-not-sent.
    //
    // 0.37.0: drain under the spawn's key only. The 0.36.14 dual-key
    // (prefixed + bare-name fallback) drain is retired now that every
    // awareness-bus enqueue carries workspace context via signal.to;
    // both ends of the queue use the same `<project_id>:<bare>` key.
    let pending = pending_live::drain_for_agent(&req.agent_name);
    let pending_drained = pending.len();
    for signal in pending {
        let bytes = signal_format::inject_bytes(&signal);
        session.write(bytes.into_bytes());
        std::thread::sleep(std::time::Duration::from_millis(150));
        session.write(b"\r".to_vec());
    }

    let total_ms = __t_total.elapsed().as_secs_f64() * 1000.0;
    log_debug!(
        "[v2-perf] side=daemon SPAWN_SUMMARY session={} agent={} reused=false total_ms={:.3} lookup_ms={:.3} dpty_spawn_ms={:.3} pending_drained={}",
        session_id_for_response,
        req.agent_name,
        total_ms,
        lookup_ms,
        dpty_spawn_ms,
        pending_drained
    );

    let out = serde_json::json!({
        "sessionId": session_id_for_response.to_string(),
        "agentName": req.agent_name,
        "cols": req.cols,
        "rows": req.rows,
        "reused": false,
    });
    HandlerResult {
        status: "200 OK",
        body: out.to_string(),
    }
}

/// Pure decision for whether `/cli/sessions/v2/close` is allowed to
/// tear a session down.
///
/// GH#22: a remote (or local) client attached over the grid-WS keeps
/// a live broadcast receiver on the session, so `subscriber_count > 0`
/// means "someone is watching this session right now". The age-out
/// reaper drives `/cli/sessions/v2/close`; if we let it unregister an
/// attached session, the last-`Arc` drop SIGHUPs the child out from
/// under the watching client. So when subscribers are attached we
/// REFUSE — unless the caller explicitly passed `force` (the operator
/// / deliberate-teardown escape hatch).
///
/// Returns `true` when the close should proceed, `false` when it must
/// be refused as still-attached.
fn close_allowed(subscriber_count: usize, force: bool) -> bool {
    force || subscriber_count == 0
}

/// Handler for `POST /cli/sessions/v2/close`.
///
/// Request body: `{"agent_name": "tab-<terminalId>", "force": false}`.
/// (`force` is optional, defaults to `false`.)
/// Response: `{"closed": true|false[, "reason": "..."]}`.
///
/// Unregisters the session from `v2_session_map`. The last `Arc`
/// drop triggers `DaemonPtySession::drop`, which closes the PTY
/// master channel; alacritty's IO thread then exits, the child
/// receives SIGHUP, and the session is cleaned up.
///
/// **GH#22 reaper guard.** Before unregistering, we check the v2
/// session's OWN broadcast subscriber count (the channel each grid-WS
/// subscribes to on attach). If a client is still attached
/// (`subscriber_count > 0`) we DO NOT kill the session — we return
/// `{"closed": false, "reason": "session still has attached clients"}`
/// instead. This is defense-in-depth so NO reaper path (the renderer's
/// age-out reaper or any other caller) can reap a session a client is
/// watching, which is exactly what killed remote PTY sessions over
/// K2 Connect. Deliberate teardown can bypass the guard with
/// `"force": true`.
///
/// Called on deliberate tab removal (see A6 wiring in
/// `src/renderer/stores/tabs.ts::removeTab`) and by the age-out
/// reaper. Component unmount does NOT call this; the session survives
/// workspace swap + Tauri restart.
pub fn handle_v2_close(body: &[u8]) -> HandlerResult {
    #[derive(serde::Deserialize)]
    struct CloseRequest {
        agent_name: String,
        #[serde(default)]
        force: bool,
    }

    let req: CloseRequest = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return HandlerResult {
                status: "400 Bad Request",
                body: format!(
                    r#"{{"error":"parse v2 CloseRequest: {}"}}"#,
                    e.to_string().replace('"', "'")
                ),
            }
        }
    };

    // GH#22 guard: refuse to reap a session a client is attached to.
    // Look up first (without unregistering) so we can read the live
    // subscriber count from the session's own broadcast channel.
    if let Some(session) = v2_session_map::lookup_by_agent_name(&req.agent_name) {
        let subscribers = session.subscriber_count();
        if !close_allowed(subscribers, req.force) {
            log_debug!(
                "[daemon/v2-close] REFUSED close for agent={} — {} attached subscriber(s), no force flag (GH#22 reaper guard)",
                req.agent_name,
                subscribers,
            );
            return HandlerResult {
                status: "200 OK",
                body: serde_json::json!({
                    "closed": false,
                    "reason": "session still has attached clients",
                    "subscriberCount": subscribers,
                })
                .to_string(),
            };
        }
    }

    let removed = v2_session_map::unregister(&req.agent_name).is_some();
    HandlerResult {
        status: "200 OK",
        body: serde_json::json!({ "closed": removed }).to_string(),
    }
}

/// Read the current `{cols, rows}` from a session's alacritty Term.
/// Used to populate the response for a reused session so the caller
/// knows the actual dimensions (which may differ from what they
/// requested if an earlier caller already sized the session).
fn current_dims(session: &DaemonPtySession) -> (u16, u16) {
    use k2_core::terminal::Dimensions;
    let term_mutex = session.term();
    let term = term_mutex.lock();
    let cols = term.columns() as u16;
    let rows = term.screen_lines() as u16;
    (cols, rows)
}

/// Subscribe to a freshly-spawned session's alacritty events on a
/// detached tokio task and call `v2_session_map::unregister(agent)`
/// when ChildExit arrives. The unregister hook is what handles the
/// DB cleanup — see `v2_session_map::unregister`. Detached because
/// we don't have a JoinHandle to track and the task is short-lived
/// (only runs until the child dies, which terminates the underlying
/// broadcast channel and ends our `recv()` loop).
///
/// Holds a Weak reference to the session so the observer task
/// doesn't keep the Arc alive past the last legitimate holder. If
/// every other holder drops first, `Weak::upgrade()` returns None
/// and we exit silently.
pub fn spawn_child_exit_observer(agent_name: String, session: std::sync::Arc<DaemonPtySession>) {
    use k2_core::terminal::AlacEvent;
    let weak = std::sync::Arc::downgrade(&session);
    drop(session);
    tokio::spawn(async move {
        // Re-acquire briefly to grab a receiver. If the session was
        // already dropped, exit — nothing to observe.
        let mut rx = match weak.upgrade() {
            Some(s) => s.subscribe_events(),
            None => return,
        };
        // Drop the temporary strong reference so we don't keep the
        // Arc alive ourselves; the receiver alone is enough.
        loop {
            match rx.recv().await {
                Ok(AlacEvent::ChildExit(status)) => {
                    log_debug!(
                        "[daemon/v2-exit] ChildExit observed for agent={} code={:?} — unregistering",
                        agent_name,
                        status.code(),
                    );
                    // Flip the session's child_exited flag so any
                    // subsequent lookup_by_agent_name caller (the
                    // spawn-helper idempotency check, the agents
                    // running reaping pass) sees the dead state
                    // immediately. Without this, the small window
                    // between ChildExit and unregister could surface
                    // a stale Arc as "live" to a fast-following
                    // lookup. Fix is cheap; race is rare; bug class
                    // is high-impact.
                    if let Some(s) = weak.upgrade() {
                        s.mark_child_exited();
                    }
                    v2_session_map::unregister(&agent_name);
                    return;
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // Each test gets its own agent_name so parallel test runs
    // don't stomp on each other's map entries.
    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
    fn uniq_agent_name() -> String {
        format!("test-v2-{}", NEXT_ID.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn spawn_request_rejects_empty_agent_name() {
        let body = br#"{"agent_name":""}"#;
        let result = handle_v2_spawn(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("agent_name required"));
    }

    #[test]
    fn spawn_request_rejects_malformed_json() {
        let body = b"not json at all";
        let result = handle_v2_spawn(body);
        assert_eq!(result.status, "400 Bad Request");
        assert!(result.body.contains("parse v2 SpawnRequest"));
    }

    #[test]
    fn close_noop_returns_closed_false() {
        let agent = uniq_agent_name();
        let body =
            format!(r#"{{"agent_name":"{}"}}"#, agent).into_bytes();
        let result = handle_v2_close(&body);
        assert_eq!(result.status, "200 OK");
        assert!(result.body.contains(r#""closed":false"#));
    }

    // GH#22 close-guard decision table (pure logic; no PTY needed).
    // The spawn-backed end-to-end variant (real session + real grid-WS
    // subscriber drives `subscriber_count`) lives in
    // crates/k2so-daemon/tests/reaper_close_guard_integration.rs.
    #[test]
    fn close_allowed_proceeds_when_no_subscribers() {
        // Nobody attached → safe to reap.
        assert!(close_allowed(0, false));
        assert!(close_allowed(0, true));
    }

    #[test]
    fn close_allowed_refuses_attached_without_force() {
        // A client is watching and no force flag → REFUSE.
        assert!(!close_allowed(1, false));
        assert!(!close_allowed(5, false));
    }

    #[test]
    fn close_allowed_force_bypasses_attached_guard() {
        // Deliberate teardown escape hatch overrides the guard.
        assert!(close_allowed(1, true));
        assert!(close_allowed(42, true));
    }

    // Full spawn-then-lookup + spawn-then-reuse tests live in
    // crates/k2so-daemon/tests/ where a running tokio runtime and
    // the ability to fork a shell are available. They gate A7's
    // parity work.
}
