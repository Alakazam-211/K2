//! H1 of Phase 4 — daemon-side `/cli/terminal/read` +
//! `/cli/terminal/write`.
//!
//! Both routes operate on sessions the daemon owns
//! (`session::registry` + `session_map`). Before Phase 4, these
//! endpoints lived in Tauri's `agent_hooks.rs` and reached into
//! `AppState.terminal_manager` — which only knew about sessions
//! Tauri had spawned, not the daemon's own pool. This module closes
//! that split: for daemon-owned sessions, the daemon answers; Tauri
//! becomes a pure client calling these endpoints over HTTP.
//!
//! **`id` parameter** — the session identifier is a UUID string
//! matching `SessionId::to_string()`. Legacy Tauri `id`s (arbitrary
//! strings assigned by `TerminalManager::create`) are NOT accepted
//! here — once Phase 5 removes alacritty, every live session is a
//! `SessionId` anyway.
//!
//! **Response shape** — matches the pre-Phase-4 Tauri responses so
//! the CLI + companion clients don't have to branch:
//!   - write: `{"success":true}` or error
//!   - read:  `{"lines":["line1","line2",...]}`

use std::collections::HashMap;
use std::time::Instant;

use k2_core::log_debug;

use k2_core::session::{registry, Frame, SessionId};

use crate::cli_response::CliResponse;
use crate::session_lookup;

/// Handler for `GET /cli/terminal/read?id=<session>&lines=<n>[&scrollback=true]`.
///
/// `<id>` accepts three forms:
///
///   1. **v2 session UUID** (`<v2 SessionId>`) — the canonical
///      workspace+agent PTY. Resolved via `v2_session_map`. Reads
///      the live alacritty `Term`'s grid (rendered TUI surface)
///      and returns the last N rows as plain text.
///   2. **Canonical workspace+agent key** (`<project_id>:<agent>`) —
///      same path as (1), looked up by `lookup_by_agent_name`.
///      Lets callers tail by name without first resolving the
///      session UUID.
///   3. **Sub-terminal SessionId** — the older `terminal spawn`
///      facility's session-stream registry. Reads the replay ring
///      (LineMux-emitted Frame::Text). Used by tooling that
///      spawned a one-off command via `terminal spawn --command`.
///
/// All three forms produce the same response shape:
/// `{"lines":["row1","row2",...]}`.
pub fn handle_read(params: &HashMap<String, String>) -> CliResponse {
    let requested_lines: usize = params
        .get("lines")
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    // 0.39.x: workspace-name addressed read — the `k2so read <workspace>`
    // surface. Resolves a workspace NAME (or path / id) the same way
    // `msg` / `inbox` do, then reads that workspace's canonical
    // (primary / coordinator) v2 session — or `<project_id>:<agent>`
    // when `agent=` is given. Lets one agent peek another agent's live
    // terminal (human-in-the-loop, diagnose a stuck agent, read before
    // inject) without first discovering a session UUID. Distinct from
    // the `id=` forms below, which address a session directly.
    if let Some(ws) = params
        .get("workspace")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    {
        let project_path = match crate::workspace_msg::resolve_workspace(ws) {
            Some(p) => p,
            None => {
                return CliResponse::bad_request(format!(
                    "unknown workspace '{ws}' (pass a name, path, or id — see `k2so connections list`)"
                ))
            }
        };
        let project_id = match crate::canonical_session::lookup_project_id(&project_path) {
            Some(id) => id,
            None => {
                return CliResponse::bad_request(format!(
                    "workspace '{ws}' has no project id on record"
                ))
            }
        };
        let agent = params
            .get("agent")
            .map(String::as_str)
            .filter(|s| !s.is_empty());
        let key = build_canonical_read_key(&project_id, agent);
        return match crate::v2_session_map::lookup_by_agent_name(&key) {
            Some(session) => read_v2_grid_lines(&session, requested_lines),
            None => CliResponse::bad_request(format!(
                "no live session for workspace '{ws}'{} — it may be asleep \
                 (check with `k2so sessions live {ws}`)",
                agent
                    .map(|a| format!(" agent '{a}'"))
                    .unwrap_or_default(),
            )),
        };
    }

    let id_str = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id or workspace param"),
    };

    // Form 2: canonical key `<project_id>:<agent>` — lookup_by_agent_name
    // accepts arbitrary string keys, so a colon-bearing id that
    // doesn't parse as a UUID is the canonical-key form.
    if id_str.contains(':')
        && k2_core::session::SessionId::parse(id_str).is_none()
    {
        if let Some(session) = crate::v2_session_map::lookup_by_agent_name(id_str) {
            return read_v2_grid_lines(&session, requested_lines);
        }
        return CliResponse::bad_request(format!(
            "no live v2 session under canonical key '{id_str}'"
        ));
    }

    // Form 1 + 3: try parsing as a UUID, then dispatch on which
    // map owns it. v2 first — that's the workspace+agent PTY's
    // primary read surface.
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => return CliResponse::bad_request("invalid session id (expected UUID or canonical key)"),
    };

    if let Some(session) = crate::v2_session_map::lookup_by_session_id(&session_id) {
        return read_v2_grid_lines(&session, requested_lines);
    }

    // Form 3 fallback: sub-terminal session-stream registry. This
    // is the older Frame::Text replay-ring path used by tooling
    // that called `terminal spawn --command "..."`. Distinct from
    // v2: those sub-terminals don't have an alacritty Term backing
    // them, just a session_stream pipeline.
    let entry = match registry::lookup(&session_id) {
        Some(e) => e,
        None => return CliResponse::bad_request(
            "session not found (checked v2_session_map + session_stream registry)",
        ),
    };

    // Decode every Frame::Text's bytes. LineMux emits a Frame::Text
    // at each commit_line / flush_pending_text boundary — the raw
    // `\n` delimiter is CONSUMED on commit and isn't included in
    // any frame's bytes. To reconstruct displayable lines we:
    //   1. Insert `\n` between Frame::Text entries (each frame ends
    //      on a commit-line or flush boundary).
    //   2. Then split on `\n` / `\r\n` so any accidentally embedded
    //      newline inside a single frame (shouldn't happen with
    //      LineMux but handled defensively) still splits cleanly.
    //
    // Non-text frames (CursorOp, SemanticEvent, AgentSignal,
    // RawPtyFrame) don't contribute to the "read" output — the
    // caller asked for displayable lines.
    let mut parts = Vec::<Vec<u8>>::new();
    for frame in entry.replay_snapshot() {
        if let Frame::Text { bytes, .. } = frame {
            parts.push(bytes);
        }
    }
    // Some producers (real LineMux today) emit Frame::Text WITHOUT
    // a trailing `\n` — it was the commit-line delimiter. Other
    // producers (tests, future producers that pass bytes through
    // verbatim) may leave `\n` in the frame's bytes. Stripping a
    // single trailing `\n` normalizes both shapes before we join
    // with `\n` between frames.
    let joined = parts
        .iter()
        .map(|p| {
            let s = String::from_utf8_lossy(p).into_owned();
            s.strip_suffix('\n').map(str::to_string).unwrap_or(s)
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Split on line terminators, preferring CRLF then LF, and
    // strip trailing '\r' that comes from a CRLF pair.
    let mut lines: Vec<String> = joined
        .split('\n')
        .map(|s| s.strip_suffix('\r').unwrap_or(s).to_string())
        .collect();
    // A trailing newline produces a final empty element; preserve
    // if present so callers can distinguish "ends on newline" from
    // "line still being typed," but don't synthesize one.
    if lines.last().map(String::is_empty).unwrap_or(false) {
        lines.pop();
    }

    let start = lines.len().saturating_sub(requested_lines);
    let tail: Vec<String> = lines[start..].to_vec();

    CliResponse::ok_json(serde_json::json!({ "lines": tail }).to_string())
}

/// Render a v2 session's live grid + scrollback as plain-text lines
/// and return the last N. Used by `handle_read` for the primary
/// "show me what's on the screen for this canonical workspace+agent
/// PTY" surface — every workspace's pinned chat tab is a v2 session,
/// so this is the read path that closes the operational-visibility
/// gap (peek before inject, diagnose stuck agents, etc.).
///
/// Reads the alacritty `Term`'s grid (the rendered TUI surface, not
/// raw byte history). One line per Term row; trailing-empty rows
/// are trimmed. Cursor / SGR styling is dropped — this is plain
/// text for human eyes / shell pipelines.
fn read_v2_grid_lines(
    session: &std::sync::Arc<k2_core::terminal::daemon_pty::DaemonPtySession>,
    requested_lines: usize,
) -> CliResponse {
    use k2_core::terminal::grid_snapshot::snapshot_term;

    // Lock the Term briefly, take a snapshot, drop the lock fast.
    // The snapshot returns owned data so we can hold it across the
    // unlock without keeping the Term blocked.
    let term_arc = session.term();
    let snapshot = {
        let term = term_arc.lock();
        snapshot_term(&session.session_id.to_string(), &*term, 0)
    };

    // Concatenate scrollback + live grid in order (scrollback
    // first = oldest at top), render each row by joining its
    // CellRun.text values.
    let mut lines: Vec<String> = Vec::with_capacity(
        snapshot.scrollback.len() + snapshot.grid.len(),
    );
    for row in &snapshot.scrollback {
        let text: String = row.iter().map(|run| run.text.as_str()).collect();
        lines.push(text.trim_end().to_string());
    }
    for row in &snapshot.grid {
        let text: String = row.iter().map(|run| run.text.as_str()).collect();
        lines.push(text.trim_end().to_string());
    }

    // Trim trailing empty rows — alacritty pads the live grid to
    // its full row count, so an idle terminal returns dozens of
    // empty rows that are noise to the caller. The first non-empty
    // row from the end is the last meaningful line.
    while lines.last().map(String::is_empty).unwrap_or(false) {
        lines.pop();
    }

    let start = lines.len().saturating_sub(requested_lines);
    let tail: Vec<String> = lines[start..].to_vec();
    CliResponse::ok_json(serde_json::json!({ "lines": tail }).to_string())
}

/// Handler for `GET /cli/sessions/resize?session=<uuid>&cols=N&rows=N`.
///
/// Resizes both the underlying PTY and the alacritty Term backing
/// the session so the child process (bash, claude, etc.) re-flows
/// its output for the new dimensions. Returns `{"success":true}`
/// on success; 400 on validation failure.
///
/// Phase 4.5 I7: Kessel's ResizeObserver fires on pane dimension
/// changes and calls this endpoint to keep the PTY in sync with
/// the DOM cells the user sees.
/// Handler for `GET /cli/sessions/label?id=<uuid>&label=<text>[&lock=true|false]`.
///
/// 0.37.4 Phase B — explicit caller-driven label set. Updates the
/// session's authoritative label and (by default) flips
/// `LabelSource` to `Locked` so future PTY title events can't
/// overwrite. Broadcasts on the session's label channel so every
/// WS subscriber emits `LabelChanged` to its client.
///
/// Only v2 sessions have labels. Legacy `SessionStreamSession`
/// (Kessel-T0) returns 400 — opt-in users on Kessel can rely on
/// PTY title flow as before.
pub fn handle_sessions_label(params: &HashMap<String, String>) -> CliResponse {
    let id_str = match params.get("id").or_else(|| params.get("session")) {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => return CliResponse::bad_request("invalid session id (expected UUID)"),
    };
    let label = match params.get("label") {
        Some(s) => s.to_string(),
        _ => return CliResponse::bad_request("missing label param"),
    };
    let lock = params
        .get("lock")
        .map(|v| !matches!(v.as_str(), "false" | "0"))
        .unwrap_or(true);

    let v2_session = match crate::v2_session_map::lookup_by_session_id(&session_id) {
        Some(s) => s,
        None => {
            return CliResponse::bad_request(
                "session not found in v2_session_map (labels are v2-only)",
            );
        }
    };

    let new_label = v2_session.set_label(label, lock);
    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "label": new_label,
            "locked": lock,
        })
        .to_string(),
    )
}

pub fn handle_sessions_resize(params: &HashMap<String, String>) -> CliResponse {
    let id_str = match params.get("session").or_else(|| params.get("id")) {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing session param"),
    };
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => {
            return CliResponse::bad_request("invalid session id (expected UUID)")
        }
    };
    let cols: u16 = match params.get("cols").and_then(|s| s.parse().ok()) {
        Some(c) if c >= 1 => c,
        _ => return CliResponse::bad_request("missing or invalid cols (>=1)"),
    };
    let rows: u16 = match params.get("rows").and_then(|s| s.parse().ok()) {
        Some(r) if r >= 1 => r,
        _ => return CliResponse::bad_request("missing or invalid rows (>=1)"),
    };

    let session = match session_lookup::lookup_by_session_id(&session_id) {
        Some(s) => s,
        None => {
            return CliResponse::bad_request(
                "session not found (checked legacy + v2 maps)",
            );
        }
    };
    if let Err(e) = session.resize(cols, rows) {
        return CliResponse::bad_request(format!("resize failed: {e}"));
    }
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// Handler for `GET /cli/terminal/write?id=<session>&message=<text>[&no_submit=true]`.
///
/// Looks up the session in the daemon's `session_map` by session
/// id, writes `message` bytes to the PTY, and (unless `no_submit`)
/// fires a follow-up `\r` 150 ms later — matches the pre-Phase-4
/// Tauri endpoint's "paste then enter" cadence. CLI LLMs treat
/// paste+Enter as a single event and swallow a trailing \r, so
/// the split-write is load-bearing.
pub fn handle_write(params: &HashMap<String, String>) -> CliResponse {
    let id_str = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => return CliResponse::bad_request("invalid session id (expected UUID)"),
    };
    let message = match params.get("message") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return CliResponse::bad_request("missing message param"),
    };
    let no_submit = params
        .get("no_submit")
        .map(|v| matches!(v.as_str(), "true" | "1"))
        .unwrap_or(false);

    let session = match session_lookup::lookup_by_session_id(&session_id) {
        Some(s) => s,
        None => {
            return CliResponse::bad_request(
                "session not found (checked legacy + v2 maps)",
            );
        }
    };

    if let Err(e) = session.write(message.as_bytes()) {
        return CliResponse::bad_request(format!("pty write failed: {e}"));
    }

    if !no_submit {
        // Follow-up Enter in a detached thread so the HTTP response
        // doesn't wait 150 ms unnecessarily.
        let session = session.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let _ = session.write(b"\r");
        });
    }

    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// Handler for `GET /cli/terminal/spawn`.
///
/// Spawns a new session on behalf of an agent via the shared
/// `spawn::spawn_agent_session` helper, then emits a
/// `CliTerminalSpawn` HookEvent so any attached Tauri UI can
/// open a pane for it. The HookEvent is observational — the
/// PTY already exists in the daemon regardless of whether a UI
/// picks it up.
///
/// Params:
///   - `agent`   — agent name (required; used as session_map key)
///   - `command` — optional shell command; default shell if absent
///   - `cwd`     — optional working directory; project path default
///   - `title`   — optional display title for the UI pane
///   - `cols` / `rows` — optional dimensions (default 80×24)
///   - `wait`    — observational hint for UI; not used daemon-side
pub fn handle_terminal_spawn(
    params: &HashMap<String, String>,
    project_path: &str,
) -> CliResponse {
    spawn_terminal_impl(
        params,
        project_path,
        k2_core::agent_hooks::HookEvent::CliTerminalSpawn,
        /* require_agent= */ true,
    )
}

/// Handler for `GET /cli/terminal/spawn-background`.
///
/// Like `/cli/terminal/spawn` but emits the `CliTerminalSpawnBackground`
/// event instead — telling UIs "this is a background / companion
/// terminal, don't steal focus." Agent param is OPTIONAL; if
/// absent, the session registers under a synthesized
/// `terminal-<short-uuid>` key so it's still addressable via
/// session_map (for H1's write + H2's listing).
pub fn handle_terminal_spawn_background(
    params: &HashMap<String, String>,
    project_path: &str,
) -> CliResponse {
    spawn_terminal_impl(
        params,
        project_path,
        k2_core::agent_hooks::HookEvent::CliTerminalSpawnBackground,
        /* require_agent= */ false,
    )
}

fn spawn_terminal_impl(
    params: &HashMap<String, String>,
    project_path: &str,
    event: k2_core::agent_hooks::HookEvent,
    require_agent: bool,
) -> CliResponse {
    use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

    let agent_param = params.get("agent").cloned().unwrap_or_default();
    if require_agent && agent_param.is_empty() {
        return CliResponse::bad_request("missing agent param");
    }
    let command = params.get("command").and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(s.clone())
        }
    });
    if command.is_none() && require_agent {
        // Foreground-style /cli/terminal/spawn used to accept an
        // empty command and emit the event without spawning — the
        // UI decided whether to open a shell. For Phase 4 we
        // unconditionally spawn, so require an explicit command.
        // Callers that want a shell pass `command=bash` (or any
        // shell of choice).
    }
    let cwd = params
        .get("cwd")
        .cloned()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| project_path.to_string());
    let cols: u16 = params
        .get("cols")
        .and_then(|s| s.parse().ok())
        .unwrap_or(80);
    let rows: u16 = params
        .get("rows")
        .and_then(|s| s.parse().ok())
        .unwrap_or(24);

    // Synthesize an agent_name for background spawns that didn't
    // supply one — must be non-empty so session_map accepts it.
    let agent_name = if agent_param.is_empty() {
        format!(
            "terminal-{}",
            &SessionId::new().to_string()[..8]
        )
    } else {
        agent_param
    };

    // Resolve project_id so the canonical-key idempotency applies
    // when the caller is a workspace-agent spawn (agent_name matches
    // the workspace's primary agent). For ad-hoc terminals (Cmd+T,
    // tab-* synthetic names), the lookup still works — there's just
    // never a conflicting prior session under the synthesized key.
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &cwd)
            .or_else(|| k2_core::workspace::agent_identity::resolve_project_id(&conn, project_path))
    };
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.clone(),
        project_id,
        cwd: cwd.clone(),
        command: command.clone(),
        args: None,
        cols,
        rows,
        canonical_key: None,
    }) {
        Ok(o) => o,
        Err(e) => return CliResponse::bad_request(format!("spawn failed: {e}")),
    };

    // Emit the HookEvent so Tauri (or any other subscriber on the
    // daemon's /events WS) can react — open a pane, show a
    // notification, etc. Same shape as the legacy Tauri endpoints
    // so existing subscribers don't need to change.
    let title = params.get("title").cloned();
    let wait = params
        .get("wait")
        .map(|v| matches!(v.as_str(), "1" | "true"))
        .unwrap_or(false);
    let payload = serde_json::json!({
        "terminalId": outcome.session_id.to_string(),
        "agentName": &agent_name,
        "command": command,
        "cwd": cwd,
        "title": title,
        "wait": wait,
        "projectPath": project_path,
    });
    k2_core::agent_hooks::emit(event, payload);

    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "terminalId": outcome.session_id.to_string(),
            "agentName": agent_name,
            "pendingDrained": outcome.pending_drained,
        })
        .to_string(),
    )
}

/// Handler for `GET /cli/agents/running`.
///
/// Returns a JSON array of every live session the daemon knows
/// about (one entry per `session_map` key). Shape per item
/// matches the pre-Phase-4 Tauri endpoint plus daemon-native
/// enrichments:
///
/// ```json
/// {
///   "terminalId": "<session-uuid>",
///   "agentName": "<name>",
///   "cwd": "<resolved-path>",
///   "command": "<shell-cmd>|null",
///   "createdAtMs": 1234567890123,
///   "idleMs": 2345,
///   "subscriberCount": 1
/// }
/// ```
///
/// Reads from `session_map::snapshot()` + `session::registry`
/// — no PTY peeking, no DB round-trip. O(N) in live sessions.
///
/// No `project` param needed: the daemon's session pool is
/// process-wide, not per-project. Phase 4 H4 adds a companion
/// endpoint that groups sessions by project.
pub fn handle_agents_running(_params: &HashMap<String, String>) -> CliResponse {
    let now = Instant::now();
    let sessions = session_lookup::snapshot_all();
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(sessions.len());
    let mut reaped: Vec<String> = Vec::new();
    for (agent_name, session) in sessions {
        // 0.37.0: defensive PID reaping. The child-exit observer
        // normally keeps v2_session_map authoritative, but if it
        // panicked or the broadcast channel closed before a
        // ChildExit landed, a dead session can linger. Detect it
        // here (cheap atomic load) and unregister + skip — the
        // operator's `agents running` query never reports zombies.
        if !session.is_child_alive() {
            reaped.push(agent_name.clone());
            crate::v2_session_map::unregister(&agent_name);
            continue;
        }
        // v2 sessions don't register in `k2_core::session::registry`
        // (only legacy session_stream_pty.rs did). Idle tracking lives
        // in the alacritty Term update path, not the registry, so idle
        // surfaces as 0 here.
        //
        // subscriberCount, however, MUST be real (GH#22): the reaper
        // and the renderer use it to tell whether a client is attached
        // to a session. The registry lookup is ALWAYS 0 for v2 sessions
        // (they never register there), so source it from the v2
        // session's OWN broadcast channel — the exact channel each
        // grid-WS subscribes to via `session.subscribe_events()` in
        // sessions_grid_ws.rs. `receiver_count()` therefore counts the
        // live attached viewers and drops to 0 when all clients detach.
        let session_id = session.session_id();
        let idle_ms = registry::lookup(&session_id)
            .map(|entry| entry.idle_for(now).as_millis() as u64)
            .unwrap_or(0);
        let subscriber_count = session.subscriber_count();
        out.push(serde_json::json!({
            "terminalId": session_id.to_string(),
            "agentName": agent_name,
            "cwd": session.cwd(),
            "command": session.command(),
            "idleMs": idle_ms,
            "subscriberCount": subscriber_count,
        }));
    }
    if !reaped.is_empty() {
        log_debug!(
            "[daemon/agents-running] reaped {} stale session(s): {:?}",
            reaped.len(),
            reaped,
        );
    }
    CliResponse::ok_json(serde_json::to_string(&out).unwrap_or_else(|_| "[]".into()))
}

/// Handler for `GET /cli/agents/reap`.
///
/// Force-reaps every v2 session whose child PID has exited, regardless
/// of whether the child-exit observer has fired yet. Returns the count
/// of reaped sessions plus their names. Operator escape hatch — the
/// canonical-session work in 0.37.0 makes accumulated zombies very
/// rare, but having an explicit verb to clear them out is cheap
/// insurance against the infrequent observer-task crash.
pub fn handle_agents_reap(_params: &HashMap<String, String>) -> CliResponse {
    let sessions = session_lookup::snapshot_all();
    let mut reaped: Vec<String> = Vec::new();
    for (agent_name, session) in sessions {
        if !session.is_child_alive() {
            crate::v2_session_map::unregister(&agent_name);
            reaped.push(agent_name);
        }
    }
    log_debug!(
        "[daemon/agents-reap] manual reap pass cleared {} session(s): {:?}",
        reaped.len(),
        reaped,
    );
    CliResponse::ok_json(
        serde_json::json!({
            "reapedCount": reaped.len(),
            "reaped": reaped,
        })
        .to_string(),
    )
}

/// Build the `v2_session_map` lookup key for a workspace-addressed
/// read (`k2so read <workspace> [--agent <name>]`).
///
/// - **No agent** → the workspace's canonical (primary / coordinator)
///   session, keyed by the bare `project_id`
///   (`canonical_session::canonical_key_for`). This is the session a
///   human-in-the-loop agent most often wants to peek.
/// - **With agent** → that specific agent's PTY, keyed
///   `<project_id>:<agent>` (the same form `handle_read`'s `id=` Form 2
///   accepts).
///
/// Pure (no DB / no lock) so the key shape is unit-testable; the DB
/// resolution (`resolve_workspace` → `lookup_project_id`) is exercised
/// by the live smoke test.
fn build_canonical_read_key(project_id: &str, agent: Option<&str>) -> String {
    match agent {
        Some(a) => format!("{project_id}:{a}"),
        None => crate::canonical_session::canonical_key_for(project_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_key_without_agent_is_bare_project_id() {
        // Canonical (primary/coordinator) session is keyed by the bare
        // project_id — must match canonical_session::canonical_key_for.
        let pid = "11111111-2222-3333-4444-555555555555";
        assert_eq!(build_canonical_read_key(pid, None), pid);
        assert_eq!(
            build_canonical_read_key(pid, None),
            crate::canonical_session::canonical_key_for(pid),
        );
    }

    #[test]
    fn read_key_with_agent_is_project_id_colon_agent() {
        let pid = "11111111-2222-3333-4444-555555555555";
        assert_eq!(
            build_canonical_read_key(pid, Some("scout")),
            format!("{pid}:scout"),
        );
    }

    #[test]
    fn read_key_with_agent_matches_handle_read_form2_shape() {
        // handle_read's Form-2 colon-key branch looks up
        // `<project_id>:<agent>` verbatim via lookup_by_agent_name, so
        // the key we build for the `--agent` path must be byte-identical
        // to what a caller would pass as `id=<project_id>:<agent>`.
        let pid = "abc";
        let key = build_canonical_read_key(pid, Some("frontend-eng"));
        assert!(key.contains(':'));
        assert_eq!(key, "abc:frontend-eng");
    }
}
