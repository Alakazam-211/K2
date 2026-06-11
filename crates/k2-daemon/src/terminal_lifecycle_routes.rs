//! Phase 2 Unit 3 — daemon-side terminal PTY lifecycle routes.
//!
//! Replaces the Tauri-side `commands/terminal.rs` surface. Each
//! handler dispatches into the process-wide
//! `k2_core::terminal::shared()` `TerminalManager` singleton — the
//! same one the daemon's existing `/cli/sessions/*` routes already
//! touch through their providers, so cross-process consistency is
//! free (there's only one TerminalManager per process; Tauri is no
//! longer a peer once Unit 3 lands).
//!
//! # Why the daemon now owns this
//!
//! Pre-Unit-3 the renderer called `invoke('terminal_create', ...)`,
//! which routed into Tauri's `commands::terminal::terminal_create`
//! which called `state.terminal_manager.lock().create(...)`. When
//! Tauri quit, the close-handler ran `manager.kill_all()` and every
//! PTY died with it. That breaks K2SO Connect (PTYs are on a remote
//! daemon; Tauri quitting locally must not kill them) and breaks
//! Mobile Companion (the daemon is the long-lived process that
//! owns the network surface — terminals should outlive the
//! foreground UI).
//!
//! Post-Unit-3 the renderer hits these daemon HTTP routes instead;
//! the daemon's `TerminalEventSink` broadcasts events as
//! `WireEvent`s on `/events`; Tauri's `daemon_events.rs` re-emits
//! them so the renderer contract (`listen('terminal:grid:<id>')`)
//! is unchanged.
//!
//! # POST vs GET
//!
//! Mutating routes (create, kill, resize, kill-foreground, scroll,
//! log) are POST with JSON bodies — per the
//! `feedback_post_only_route_guards` pattern. Read routes
//! (active-count, foreground-cmd, exists, get-grid) are GET — no
//! body, idempotent.
//!
//! Each POST handler in `main.rs` must include the explicit
//! method-gate `if !is_post { return 405 }` guard; see the
//! `/cli/llm/chat` pattern for the canonical shape.

use serde::Deserialize;

use crate::cli_response::CliResponse;
use crate::terminal_event_sink;

/// POST /cli/terminal/create body. Mirrors the pre-Unit-3 Tauri
/// command parameters (camelCase fields aligned with how the
/// renderer constructs the JSON body via daemonCliPost).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateBody {
    /// Pre-existing terminal id (typically a v4 UUID the renderer
    /// generated). Required so the renderer can reattach
    /// deterministically across reloads — must match the existing
    /// Tauri behavior where the renderer always supplied the id.
    id: String,
    cwd: String,
    command: Option<String>,
    args: Option<Vec<String>>,
    cols: Option<u16>,
    rows: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct IdBody {
    id: String,
}

#[derive(Debug, Deserialize)]
struct ResizeBody {
    id: String,
    cols: u16,
    rows: u16,
}

#[derive(Debug, Deserialize)]
struct ScrollBody {
    id: String,
    delta: i32,
}

#[derive(Debug, Deserialize)]
struct LogBody {
    message: String,
}

#[derive(Debug, Deserialize)]
struct WriteBody {
    id: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct SetFocusBody {
    id: String,
    focused: bool,
}

/// POST /cli/terminal/create — spawn a PTY managed by the
/// process-wide `TerminalManager`. Returns `{"id": "<id>"}`
/// (matches the Tauri command's response shape).
///
/// Blocking work: posix_spawn + alacritty Term init + worker thread
/// startup. Callers wrap in `tokio::task::spawn_blocking` (F5).
pub fn handle_create(body_bytes: &[u8]) -> CliResponse {
    let body: CreateBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };

    let sink = match terminal_event_sink::build_sink() {
        Some(s) => s,
        None => {
            return CliResponse::internal_error(
                "terminal_event_sink::register has not run yet".to_string(),
            );
        }
    };

    let manager = k2_core::terminal::shared();
    let mut manager = manager.lock();
    match manager.create(
        body.id.clone(),
        body.cwd,
        body.command,
        body.args,
        body.cols,
        body.rows,
        sink,
    ) {
        Ok(()) => CliResponse::ok_json(
            serde_json::json!({ "id": body.id }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/terminal/kill — terminate the PTY. Returns
/// `{"success":true}` on success.
pub fn handle_kill(body_bytes: &[u8]) -> CliResponse {
    let body: IdBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let manager = k2_core::terminal::shared();
    let mut manager = manager.lock();
    match manager.kill(&body.id) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/terminal/resize — change PTY dimensions.
pub fn handle_resize(body_bytes: &[u8]) -> CliResponse {
    let body: ResizeBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    match manager.resize(&body.id, body.cols, body.rows) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/terminal/kill-foreground — SIGINT the foreground
/// process (Ctrl-C semantics) without killing the shell itself.
/// Unix-only; on non-unix returns a clean error.
pub fn handle_kill_foreground(body_bytes: &[u8]) -> CliResponse {
    #[cfg(unix)]
    {
        let body: IdBody = match serde_json::from_slice(body_bytes) {
            Ok(b) => b,
            Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
        };
        let manager = k2_core::terminal::shared();
        let manager = manager.lock();
        match manager.kill_foreground(&body.id) {
            Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
            Err(e) => CliResponse::bad_request(e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = body_bytes;
        CliResponse::bad_request("kill-foreground is unix-only".to_string())
    }
}

/// POST /cli/terminal/scroll — scroll the terminal's viewport by
/// `delta` lines (positive = scroll back, negative = scroll forward
/// toward the prompt).
pub fn handle_scroll(body_bytes: &[u8]) -> CliResponse {
    let body: ScrollBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    match manager.scroll(&body.id, body.delta) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/terminal/log — optional debug logging from the
/// renderer. Returns `{"success":true}`. Body: `{"message":"..."}`.
pub fn handle_log(body_bytes: &[u8]) -> CliResponse {
    let body: LogBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    k2_core::log_debug!("[terminal/renderer] {}", body.message);
    CliResponse::ok_json(r#"{"success":true}"#.to_string())
}

/// POST /cli/terminal/lifecycle-write — byte-level PTY write for the
/// lifecycle-managed terminals (NOT the v2 session_map ones). The
/// existing `/cli/terminal/write` route (in `terminal_routes.rs`)
/// is session_map-based and assumes a UUID SessionId. The legacy
/// arbitrary-string TerminalManager IDs don't fit there; this route
/// is the parallel path for those.
///
/// Body: `{"id": "...", "data": "..."}`. The data string is sent
/// as bytes via `TerminalManager::write`.
pub fn handle_lifecycle_write(body_bytes: &[u8]) -> CliResponse {
    let body: WriteBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    match manager.write(&body.id, &body.data) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// POST /cli/terminal/set-focus — mark a terminal focused/unfocused.
/// The underlying TerminalManager treats this as a no-op (focus
/// state is tracked but doesn't drive any backend behavior); the
/// route exists for renderer parity until the renderer drops the
/// call entirely.
pub fn handle_set_focus(body_bytes: &[u8]) -> CliResponse {
    let body: SetFocusBody = match serde_json::from_slice(body_bytes) {
        Ok(b) => b,
        Err(e) => return CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    match manager.set_focus(&body.id, body.focused) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

// ── GET handlers (read-only; no method gate needed) ──────────────

/// GET /cli/terminal/active-count?path=<workspace>. Returns the
/// integer count as `{"count": N}`.
pub fn handle_active_count(
    params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    let path = match params.get("path") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing path param"),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    let count = manager.get_count_for_path(path);
    CliResponse::ok_json(serde_json::json!({ "count": count }).to_string())
}

/// GET /cli/terminal/foreground-cmd?id=<id>. Returns
/// `{"command": "<name>"|null}`.
pub fn handle_foreground_cmd(
    params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    let id = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    match manager.get_foreground_command(id) {
        Ok(opt) => CliResponse::ok_json(
            serde_json::json!({ "command": opt }).to_string(),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/terminal/exists?id=<id>. Returns `{"exists": true|false}`.
pub fn handle_exists(
    params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    let id = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    let exists = manager.exists(id);
    CliResponse::ok_json(serde_json::json!({ "exists": exists }).to_string())
}

/// GET /cli/terminal/get-grid?id=<id>. Returns the full GridUpdate
/// (serialized via core's existing Serialize impl). Used by the
/// renderer's reattach path to draw the current screen before any
/// live grid delta arrives.
pub fn handle_get_grid(
    params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    let id = match params.get("id") {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return CliResponse::bad_request("missing id param"),
    };
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    match manager.get_grid(id) {
        Ok(grid) => match serde_json::to_string(&grid) {
            Ok(json) => CliResponse::ok_json(json),
            Err(e) => CliResponse::internal_error(format!("serialize grid: {e}")),
        },
        Err(e) => CliResponse::bad_request(e),
    }
}

/// GET /cli/terminal/list-running. Replaces the Tauri
/// `terminal_list_running_agents` invoke. Returns a JSON array of
/// `{"terminalId","cwd","command"}` for every live TerminalManager
/// PTY.
///
/// Distinct from `/cli/agents/running` which iterates the
/// v2_session_map (workspace-agent PTYs); this iterates the legacy
/// `TerminalManager` terminals — the renderer's "Running Agents"
/// panel needs both eventually but this route surfaces the legacy
/// list verbatim so the panel keeps working post-Unit-3.
pub fn handle_list_running(
    _params: &std::collections::HashMap<String, String>,
) -> CliResponse {
    let manager = k2_core::terminal::shared();
    let manager = manager.lock();
    let terminal_ids = manager.list_terminal_ids();
    let mut agents = Vec::with_capacity(terminal_ids.len());
    for (id, cwd) in &terminal_ids {
        let command = manager.get_foreground_command(id).ok().flatten();
        agents.push(serde_json::json!({
            "terminalId": id,
            "cwd": cwd,
            "command": command,
        }));
    }
    CliResponse::ok_json(serde_json::to_string(&agents).unwrap_or_else(|_| "[]".into()))
}
