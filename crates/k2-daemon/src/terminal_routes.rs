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

use k2_core::brand::PRODUCT_NAME;
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
///
/// `lines=` is capped ([`MAX_SCREEN_LINES`] for a screen,
/// [`MAX_REPLAY_LINES`] for a sub-terminal's replay ring). A larger number
/// is clamped, never refused: the answer is the last `max` lines, with
/// `capped: {asked, max}` and a `note` saying so.
pub fn handle_read(params: &HashMap<String, String>) -> CliResponse {
    let requested_lines = requested_lines(params);
    match read_screen(params, requested_lines) {
        Ok(read) => {
            let mut body = serde_json::json!({ "lines": read.lines });
            if requested_lines > read.max {
                body["capped"] = serde_json::json!({ "asked": requested_lines, "max": read.max });
                body["note"] = serde_json::json!(format!(
                    "lines is capped at {} for this terminal; these are its last {} lines",
                    read.max, read.max
                ));
            }
            CliResponse::ok_json(body.to_string())
        }
        Err(resp) => resp,
    }
}

/// The most lines one read returns from a terminal's screen: its whole
/// scrollback (the live rows below it come on top).
pub const MAX_SCREEN_LINES: usize = k2_core::terminal::daemon_pty::SCROLLBACK_CAP;

/// The most lines one read returns from an older sub-terminal's replay ring
/// (`terminal spawn --command`), which keeps up to
/// [`k2_core::session::REPLAY_CAP`] frames.
pub const MAX_REPLAY_LINES: usize = 10_000;

/// `lines=` as asked: 50 when absent or not a number; a number too large
/// for this machine counts as the largest (it is clamped anyway).
fn requested_lines(params: &HashMap<String, String>) -> usize {
    params
        .get("lines")
        .and_then(|s| s.trim().parse::<u128>().ok())
        .map(|n| usize::try_from(n).unwrap_or(usize::MAX))
        .unwrap_or(50)
}

/// Shared write/read gate for remote-session-bound PTYs.
/// Returns Some(403) to deny, None to allow (or when session is not remote-bound).
fn gate_remote_io_if_bound(
    session_id: &str,
    params: &HashMap<String, String>,
) -> Option<CliResponse> {
    let presented = params.get("token").map(String::as_str);
    // Owner identity for the gate:
    // 1. Production: hook_config token set at daemon boot.
    // 2. Tests / fallback: if the presented token is neither a grant, a
    //    connect-user session nor a session passport, it is the owner
    //    credential (the dispatcher admitted nothing else).
    let hook = k2_core::hook_config::get_token();
    let owner_buf: String = if !hook.is_empty() {
        hook.to_string()
    } else {
        match presented {
            Some(t)
                if !t.is_empty()
                    && !k2_core::remote_sessions::is_grant_token(t)
                    && k2_core::connect_users::validate_session(t).is_none()
                    && crate::session_token::validate_hook(t).is_none() =>
            {
                t.to_string()
            }
            _ => String::new(),
        }
    };
    crate::remote_session_routes::gate_remote_session_io(
        session_id,
        presented,
        &owner_buf,
    )
}

/// Resolve a session-addressing param set (`workspace`[+`agent`] /
/// `id` / `session` / canonical key) to the last `requested_lines` of
/// that session's rendered screen text — the SAME text `/cli/terminal/read`
/// returns. Shared by `handle_read` and `classify_routes::handle_classify`
/// so the classifier scores exactly what a human/agent sees.
///
/// `Ok(lines)` = resolved screen tail. `Err(resp)` = a `400` describing
/// why the session couldn't be addressed (unknown workspace, no live
/// session, bad UUID, …).
pub(crate) fn resolve_screen_lines(
    params: &HashMap<String, String>,
    requested_lines: usize,
) -> Result<Vec<String>, CliResponse> {
    read_screen(params, requested_lines).map(|read| read.lines)
}

/// A read's answer: the lines, and the cap that applied to this terminal.
struct ScreenRead {
    lines: Vec<String>,
    max: usize,
}

/// [`resolve_screen_lines`] that also says the cap (`requested_lines` is
/// clamped to it).
fn read_screen(
    params: &HashMap<String, String>,
    requested_lines: usize,
) -> Result<ScreenRead, CliResponse> {
    // What the address itself names, before any session is looked up: a
    // workspace (`workspace=`, `id=<project_id>:<agent>`) and the id it
    // gives. A scoped caller that may not read that workspace learns
    // nothing more (not even whether a session is live there).
    let named = params
        .get("id")
        .or_else(|| params.get("session"))
        .map(String::as_str)
        .filter(|s| !s.is_empty());
    let named_workspace = address_workspace(params).map(|ws| vec![ws]);
    may_read_session(
        params,
        &ReadTarget { session: named, workspaces: named_workspace.as_deref() },
    )?;
    // Then the session the read resolved to, by its own id and workspaces,
    // whichever form named it.
    let screen = resolve_screen(params)?;
    let id = screen.id().to_string();
    let workspaces = session_workspaces(&screen);
    may_read_session(
        params,
        &ReadTarget { session: Some(id.as_str()), workspaces: Some(workspaces.as_slice()) },
    )?;
    let max = screen.max_lines();
    Ok(ScreenRead { lines: screen.lines(requested_lines.min(max)), max })
}

/// What a terminal read names, as far as it is known: the session's id and
/// the workspaces (`projects.id`) the session belongs to. `None`: not known
/// yet (the read hasn't resolved that far).
pub(crate) struct ReadTarget<'a> {
    pub session: Option<&'a str>,
    pub workspaces: Option<&'a [String]>,
}

/// Who may read a terminal's screen, whichever way the read names it: a
/// session id (`id=` / `session=`), the canonical `<project_id>[:<agent>]`
/// key, or `workspace=` (a name, path, id, sidecar or session id). The one
/// check every form goes through.
///
/// - An App pass never reads a terminal.
/// - A remote-session shell is read only with its own grant or the owner
///   token, and a grant reads only its own shell.
/// - A workspace-scoped caller (an agent's passport, a job run's, a cell's)
///   reads a session only when every workspace it belongs to is the
///   caller's own or a local connection (the rule `workspace=` had). A
///   session K2 can't place in any workspace is refused.
/// - The owner token, a login and other unscoped callers: no workspace
///   check (the current behaviour).
pub(crate) fn may_read_session(
    params: &HashMap<String, String>,
    target: &ReadTarget<'_>,
) -> Result<(), CliResponse> {
    if params
        .get("token")
        .is_some_and(|t| k2_core::skin::is_skin_token(t) && !k2_core::skin::is_api_key_prefix(t))
    {
        return Err(crate::skin_routes::skin_terminal_forbidden());
    }
    if let Some(sid) = target.session {
        if let Some(resp) = gate_remote_io_if_bound(sid, params) {
            return Err(resp);
        }
    }
    let Some(principal) = crate::caller_workspace::principal_from_params(params) else {
        return Ok(());
    };
    let Some(workspaces) = target.workspaces else {
        return Ok(());
    };
    if workspaces.is_empty() {
        return Err(unplaced_session());
    }
    for ws in workspaces {
        match crate::comms::gate_cross_workspace(Some(&principal), ws) {
            Ok(()) => {}
            Err(Some(resp)) => return Err(resp),
            Err(None) => return Err(unplaced_session()),
        }
    }
    Ok(())
}

/// A scoped caller asked for a session K2 can't place in a workspace.
fn unplaced_session() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({
            "ok": false,
            "error": {
                "code": "forbidden",
                "hint": format!(
                    "{PRODUCT_NAME} can't tell which workspace this terminal belongs to, so it can only be read by the owner or a login"
                ),
            },
        })
        .to_string(),
    }
}

/// The workspace a read's address names before any session is looked up:
/// `workspace=<name|path|id|ws/sidecar>`, or the project in
/// `id=<project_id>:<agent>`. `None`: the address names no workspace K2 has.
fn address_workspace(params: &HashMap<String, String>) -> Option<String> {
    if let Some(ws) = params
        .get("workspace")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
    {
        return crate::comms::resolve_target_project(ws).map(|(_, id)| id);
    }
    let id = params
        .get("id")
        .or_else(|| params.get("session"))
        .map(String::as_str)
        .filter(|s| !s.is_empty())?;
    if id.contains(':') && SessionId::parse(id).is_none() {
        return key_project(id);
    }
    None
}

/// The project a session-map key names (`<project_id>` or
/// `<project_id>:<agent>`), when that project exists.
fn key_project(key: &str) -> Option<String> {
    let pid = key.split(':').next().map(str::trim).filter(|p| !p.is_empty())?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row("SELECT id FROM projects WHERE id = ?1", [pid], |r| r.get::<_, String>(0))
        .ok()
}

/// The workspaces a live session belongs to: its passport's, its session-map
/// key's and its folder's (the longest registered project or worktree that
/// holds its cwd).
fn session_workspaces(screen: &Screen) -> Vec<String> {
    fn add(out: &mut Vec<String>, ws: String) {
        if !ws.trim().is_empty() && !out.contains(&ws) {
            out.push(ws);
        }
    }
    let id = screen.id();
    let mut out = Vec::new();
    if let Some(ws) = crate::session_token::workspace_for_session(&id.to_string()) {
        add(&mut out, ws);
    }
    if let Some(ws) = crate::v2_session_map::agent_name_for_session_id(&id)
        .as_deref()
        .and_then(key_project)
    {
        add(&mut out, ws);
    }
    if let Screen::V2(session) = screen {
        if let Some(cwd) = session.cwd.as_ref() {
            if let Some((ws, _)) = crate::git_notes_routes::owning_project(&cwd.to_string_lossy()) {
                add(&mut out, ws);
            }
        }
    }
    out
}

/// The session a read resolved to.
enum Screen {
    /// A workspace+agent PTY (its live `Term`).
    V2(std::sync::Arc<k2_core::terminal::daemon_pty::DaemonPtySession>),
    /// A `terminal spawn` sub-terminal (its replay ring).
    Stream(SessionId, std::sync::Arc<k2_core::session::SessionEntry>),
}

impl Screen {
    fn id(&self) -> SessionId {
        match self {
            Screen::V2(session) => session.session_id,
            Screen::Stream(id, _) => *id,
        }
    }

    /// The most lines one read returns from this terminal.
    fn max_lines(&self) -> usize {
        match self {
            Screen::V2(_) => MAX_SCREEN_LINES,
            Screen::Stream(..) => MAX_REPLAY_LINES,
        }
    }

    fn lines(&self, requested_lines: usize) -> Vec<String> {
        match self {
            Screen::V2(session) => v2_grid_lines(session, requested_lines),
            Screen::Stream(_, entry) => stream_lines(entry, requested_lines),
        }
    }
}

/// Resolve the session a read's params name (see [`resolve_screen_lines`]).
fn resolve_screen(params: &HashMap<String, String>) -> Result<Screen, CliResponse> {
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
        // The workspace check for this form runs in
        // `resolve_screen_lines` (`may_read_session`), like every other form.
        // `sales/1` → that sidecar session (D14). Session UUID first-arg
        // is handled below via session=.
        if let Some(crate::workspace_msg::MsgTarget::Sidecar {
            path,
            conversation_key,
            ..
        }) = crate::workspace_msg::resolve_msg_target(ws)
        {
            // TR21: the sidecar's own workspace first. One provider id can
            // sit in two workspaces (a fork tab), and the project-blind
            // scan read whichever live session the map yielded first.
            let project_id = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
            };
            if let Some(project_id) = project_id {
                if let Some(live) =
                    crate::workspace_msg::lookup_live_for_sidecar(&project_id, &conversation_key)
                {
                    return Ok(Screen::V2(live.0));
                }
            }
            return Err(CliResponse::bad_request(format!(
                "no live session for sidecar '{ws}' — it may be asleep (see `k2 sessions --all`; `k2 msg <address>` wakes it)"
            )));
        }
        if let Some(crate::workspace_msg::MsgTarget::Session { session_id }) =
            crate::workspace_msg::resolve_msg_target(ws)
        {
            if let Some(sid) = k2_core::session::SessionId::parse(&session_id) {
                if let Some(session) = crate::v2_session_map::lookup_by_session_id(&sid) {
                    return Ok(Screen::V2(session));
                }
            }
        }
        let project_path = match crate::workspace_msg::resolve_workspace(ws) {
            Some(p) => p,
            None => {
                return Err(CliResponse::bad_request(format!(
                    "unknown workspace '{ws}' (pass a name, path, or id — see `k2 connections list`)"
                )))
            }
        };
        let project_id = match crate::canonical_session::lookup_project_id(&project_path) {
            Some(id) => id,
            None => {
                return Err(CliResponse::bad_request(format!(
                    "workspace '{ws}' has no project id on record"
                )))
            }
        };
        let agent = params
            .get("agent")
            .map(String::as_str)
            .filter(|s| !s.is_empty());
        let key = build_canonical_read_key(&project_id, agent);
        return match crate::v2_session_map::lookup_by_agent_name(&key) {
            Some(session) => Ok(Screen::V2(session)),
            None => Err(CliResponse::bad_request(format!(
                "no live session for workspace '{ws}'{} — it may be asleep \
                 (see who is there with `k2 sessions`)",
                agent
                    .map(|a| format!(" agent '{a}'"))
                    .unwrap_or_default(),
            ))),
        };
    }

    // Accept both `id=` (the historical read param) and `session=`
    // (the param name the classify endpoint advertises) so the two
    // routes are addressable identically.
    let id_str = match params
        .get("id")
        .or_else(|| params.get("session"))
    {
        Some(s) if !s.is_empty() => s.as_str(),
        _ => return Err(CliResponse::bad_request("missing id, session, or workspace param")),
    };

    // Form 2: canonical key `<project_id>:<agent>` — lookup_by_agent_name
    // accepts arbitrary string keys, so a colon-bearing id that
    // doesn't parse as a UUID is the canonical-key form.
    if id_str.contains(':')
        && k2_core::session::SessionId::parse(id_str).is_none()
    {
        if let Some(session) = crate::v2_session_map::lookup_by_agent_name(id_str) {
            return Ok(Screen::V2(session));
        }
        return Err(CliResponse::bad_request(format!(
            "no live v2 session under canonical key '{id_str}'"
        )));
    }

    // Form 1 + 3: try parsing as a UUID, then dispatch on which
    // map owns it. v2 first — that's the workspace+agent PTY's
    // primary read surface.
    let session_id = match SessionId::parse(id_str) {
        Some(id) => id,
        None => return Err(CliResponse::bad_request("invalid session id (expected UUID or canonical key)")),
    };

    if let Some(session) = crate::v2_session_map::lookup_by_session_id(&session_id) {
        return Ok(Screen::V2(session));
    }

    // Form 3 fallback: sub-terminal session-stream registry. This
    // is the older Frame::Text replay-ring path used by tooling
    // that called `terminal spawn --command "..."`. Distinct from
    // v2: those sub-terminals don't have an alacritty Term backing
    // them, just a session_stream pipeline.
    let entry = match registry::lookup(&session_id) {
        Some(e) => e,
        None => return Err(CliResponse::bad_request(
            "session not found (checked v2_session_map + session_stream registry)",
        )),
    };

    Ok(Screen::Stream(session_id, entry))
}

/// The last `requested_lines` of a sub-terminal's replay ring as text.
fn stream_lines(entry: &k2_core::session::SessionEntry, requested_lines: usize) -> Vec<String> {
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
    lines[start..].to_vec()
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
/// Pure helper: render a v2 session's grid + scrollback to plain-text
/// lines and return the last `requested_lines`. Split out of
/// `read_v2_grid_lines` so `classify_routes` can read the SAME text a
/// human/agent sees via `/cli/terminal/read` without re-wrapping a
/// `CliResponse`.
fn v2_grid_lines(
    session: &std::sync::Arc<k2_core::terminal::daemon_pty::DaemonPtySession>,
    requested_lines: usize,
) -> Vec<String> {
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
    lines[start..].to_vec()
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

/// S7a pin-to-size (prd-presence-multiplayer-v1 §5.5) — the wire body
/// for `POST /cli/terminal/pin-size`. Two shapes:
///   pin:   `{"session":"<uuid>","cols":100,"rows":30}`
///   unpin: `{"session":"<uuid>","clear":true}`
#[derive(Debug, serde::Deserialize)]
struct PinSizeBody {
    session: String,
    cols: Option<u16>,
    rows: Option<u16>,
    #[serde(default)]
    clear: bool,
}

/// Validation bounds for a pin. Wider than any sane preset (the S7b
/// dropdown tops out at 160×48) but tight enough that a typo'd or
/// hostile value can't blow up Term reflow / snapshot payloads.
/// `pub(crate)`: the grid-WS `claim_pin` action (companion T0)
/// enforces the SAME bounds so the two pin entry points can't drift.
pub(crate) const PIN_COLS_MIN: u16 = 20;
pub(crate) const PIN_COLS_MAX: u16 = 500;
pub(crate) const PIN_ROWS_MIN: u16 = 5;
pub(crate) const PIN_ROWS_MAX: u16 = 200;

/// Handler for `POST /cli/terminal/pin-size` (S7a).
///
/// Pins (or unpins) a v2 session's PTY to fixed cols×rows:
/// applies LIVE via [`DaemonPtySession::set_pinned`] (whose
/// `request_resize` clamp then holds the grid there against every
/// resize path) AND persists to `workspace_tab_sessions`
/// (migration 0065) so the pin survives daemon restart —
/// `v2_session_map::register` re-applies it on respawn.
///
/// `set_by` is resolved by the dispatcher from the auth token
/// ("owner" | connect-user username — never the request body, D3).
/// Method + auth gates live in the dispatcher arm (require_post +
/// token_ok, per the POST-only house rule); role tightening to
/// claimer-capable users lands with S4/S5.
///
/// Responses:
///   200 `{"success":true,"pinned":{"cols":N,"rows":N,"setBy":"…"},"persisted":bool}`
///   200 `{"success":true,"pinned":null,"persisted":bool}` (cleared)
///   400 on malformed body / unknown session / out-of-bounds dims.
pub fn handle_pin_size(body: &[u8], set_by: &str) -> CliResponse {
    let req: PinSizeBody = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => {
            return CliResponse::bad_request(format!(
                "parse pin-size body: {e}"
            ))
        }
    };
    let session_id = match SessionId::parse(&req.session) {
        Some(id) => id,
        None => {
            return CliResponse::bad_request(
                "invalid session id (expected UUID)",
            )
        }
    };
    // Resolve the session AND its map key (the key drives the
    // persistence row lookup below).
    let entry = crate::v2_session_map::snapshot()
        .into_iter()
        .find(|(_, s)| s.session_id == session_id);
    let Some((agent_name, session)) = entry else {
        return CliResponse::bad_request(
            "session not found in v2_session_map (pin-size is v2-only)",
        );
    };

    if req.clear {
        k2_core::terminal::DaemonPtySession::set_pinned(&session, None, None);
        let persisted = persist_pin(&agent_name, &session, None);
        return CliResponse::ok_json(
            serde_json::json!({
                "success": true,
                "pinned": serde_json::Value::Null,
                "persisted": persisted,
            })
            .to_string(),
        );
    }

    let (cols, rows) = match (req.cols, req.rows) {
        (Some(c), Some(r)) => (c, r),
        _ => {
            return CliResponse::bad_request(
                "missing cols/rows (or pass \"clear\":true to unpin)",
            )
        }
    };
    if !(PIN_COLS_MIN..=PIN_COLS_MAX).contains(&cols)
        || !(PIN_ROWS_MIN..=PIN_ROWS_MAX).contains(&rows)
    {
        return CliResponse::bad_request(format!(
            "cols/rows out of bounds (cols {PIN_COLS_MIN}..={PIN_COLS_MAX}, \
             rows {PIN_ROWS_MIN}..={PIN_ROWS_MAX}; got {cols}x{rows})"
        ));
    }

    k2_core::terminal::DaemonPtySession::set_pinned(
        &session,
        Some((cols, rows)),
        Some(set_by.to_string()),
    );
    let persisted = persist_pin(&agent_name, &session, Some((cols, rows, set_by)));
    CliResponse::ok_json(
        serde_json::json!({
            "success": true,
            "pinned": { "cols": cols, "rows": rows, "setBy": set_by },
            "persisted": persisted,
        })
        .to_string(),
    )
}

/// Persist a pin (or its clearing) to `workspace_tab_sessions`,
/// keyed the same way `v2_session_map::register` stamps rows:
/// project_id resolved from the session's cwd, pane_group_id from
/// the map key (`tab-<pgid>` → `<pgid>`, else the key itself —
/// which covers the canonical pinned chat's bare project_id key).
/// Returns whether a row was written; a session whose cwd isn't a
/// registered workspace pins LIVE-ONLY (no row → no restore), which
/// the route surfaces as `"persisted": false` rather than failing
/// the live pin.
fn persist_pin(
    agent_name: &str,
    session: &std::sync::Arc<k2_core::terminal::daemon_pty::DaemonPtySession>,
    pin: Option<(u16, u16, &str)>,
) -> bool {
    let Some(cwd) = session
        .cwd
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
    else {
        return false;
    };
    let db = k2_core::db::shared();
    let conn = db.lock();
    let Some(project_id) =
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &cwd)
    else {
        log_debug!(
            "[pin-size] session={} cwd={} resolves to no workspace; pin is live-only",
            session.session_id,
            cwd,
        );
        return false;
    };
    let pane_group_id = crate::session_events::pane_group_id_from_agent(agent_name)
        .unwrap_or_else(|| agent_name.to_string());
    match k2_core::db::schema::WorkspaceTabSession::set_pinned_size(
        &conn,
        &project_id,
        &pane_group_id,
        agent_name,
        Some(&cwd),
        pin,
    ) {
        Ok(()) => true,
        Err(e) => {
            log_debug!(
                "[pin-size] persist failed for session={} (project={} pane_group={}): {e}",
                session.session_id,
                project_id,
                pane_group_id,
            );
            false
        }
    }
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

/// Handler for `POST /cli/terminal/write?id=<session>&message=<text>[&no_submit=true]`.
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
    // Stage 3: grant-bound remote shells require matching grant (or owner).
    if let Some(resp) = gate_remote_io_if_bound(id_str, params) {
        return resp;
    }
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

    // DA26 / A15: a client's lone Esc / Ctrl-C feeds interrupt inference.
    crate::activity_store::note_client_input(&session_id.to_string(), message.as_bytes());
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

/// Handler for `POST /cli/terminal/spawn` (POST-only since 0.44.4).
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

/// Handler for `POST /cli/terminal/spawn-background` (POST-only since 0.44.4).
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

/// Whether the `sandbox` param asks for a sandbox. No param or an explicit
/// no (`false`, `0`, `no`, `off`, empty) is a plain spawn; any other value is
/// a request (fail closed).
fn terminal_sandbox_requested(params: &HashMap<String, String>) -> bool {
    params
        .get("sandbox")
        .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "" | "false" | "0" | "no" | "off"))
        .unwrap_or(false)
}

/// `(program, args)` for a terminal spawn's `command` (0.46.18): one word
/// is the program with no args, exactly as before (a bare shell name still
/// gets `exempt_shell_path` in the spawn); a command line is the login
/// shell with `-lc <command line>` ([`k2_core::terminal::command_line`]).
fn spawn_command_parts(command: Option<&str>) -> (Option<String>, Option<Vec<String>>) {
    match command.and_then(k2_core::terminal::command_line::spawn_parts) {
        Some((shell, args)) => (Some(shell), Some(args)),
        None => (command.map(str::to_string), None),
    }
}

fn spawn_terminal_impl(
    params: &HashMap<String, String>,
    project_path: &str,
    event: k2_core::agent_hooks::HookEvent,
    require_agent: bool,
) -> CliResponse {
    use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

    // `sandbox=true` (`k2 terminal spawn --sandbox`): a real sandbox or a
    // 409 before anything runs. Before 0.46.17 the route ignored the flag
    // and the session ran unsandboxed on every build.
    let sandbox_requested = terminal_sandbox_requested(params);
    if sandbox_requested && !crate::v2_spawn::can_sandbox() {
        return CliResponse {
            status: "409 Conflict",
            content_type: "application/json",
            body: crate::v2_spawn::sandbox_unavailable_body("--sandbox"),
        };
    }
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
    // 0.46.18: a command line (`/bin/echo hi`, a pipe, quotes) runs as
    // `<login shell> -lc '<command line>'`; before, the whole string was the
    // program and the spawn failed with ENOENT. One word starts as given.
    let (spawn_program, spawn_args) = spawn_command_parts(command.as_deref());
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
    let (terminal_id, pending_drained, sandbox_in_force) = if sandbox_requested {
        // The same door as `/cli/sessions/v2/spawn {"sandbox":true}`: a
        // microVM cell, or that door's own 409 with nothing spawned.
        // A command line runs under the guest's `/bin/sh` (the host's
        // passwd shell may not exist in the cell).
        let (sandbox_command, sandbox_args) = match command.as_deref() {
            Some(c) if k2_core::terminal::command_line::is_command_line(c) => {
                let (p, a) = k2_core::terminal::command_line::wrap_in(
                    k2_core::terminal::command_line::FALLBACK_SHELL,
                    c,
                );
                (Some(p), Some(a))
            }
            _ => (command.clone(), None),
        };
        let body = serde_json::json!({
            "agent_name": agent_name,
            "cwd": cwd,
            "command": sandbox_command,
            "args": sandbox_args,
            "cols": cols,
            "rows": rows,
            "sandbox": true,
        });
        let r = crate::v2_spawn::handle_v2_spawn(body.to_string().as_bytes());
        if r.status != "200 OK" {
            return CliResponse { status: r.status, content_type: "application/json", body: r.body };
        }
        let v: serde_json::Value = match serde_json::from_str(&r.body) {
            Ok(v) => v,
            Err(e) => return CliResponse::bad_request(format!("sandboxed spawn answer: {e}")),
        };
        let sid = v["sessionId"].as_str().unwrap_or_default().to_string();
        let backend = v["sandbox"].as_str().unwrap_or("none").to_string();
        (sid, 0, backend)
    } else {
        let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
            agent_name: agent_name.clone(),
            project_id,
            cwd: cwd.clone(),
            command: spawn_program,
            args: spawn_args,
            cols,
            rows,
            canonical_key: None,
            // Ad-hoc terminal (caller-supplied command, no preset
            // resolution) — no preset env to merge.
            env: Default::default(),
            env_owner_set: false,
            launch_prompt: None,
            label: None,
        }) {
            Ok(o) => o,
            Err(e) => return crate::folder_trust::spawn_failed_response(&e),
        };
        (outcome.session_id.to_string(), outcome.pending_drained, "none".to_string())
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
        "terminalId": &terminal_id,
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
            "terminalId": terminal_id,
            "agentName": agent_name,
            "pendingDrained": pending_drained,
            // What is in force: "microvm" for a sandbox, "none" for a plain
            // PTY that runs as the daemon's user (0.46.17).
            "sandbox": sandbox_in_force,
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
        // session's OWN viewer registry — each grid-WS connection
        // holds a `ViewerRegistration` (`session.attach_viewer()` in
        // sessions_grid_ws.rs) for its lifetime, so the count is the
        // live attached viewers and drops to 0 when all clients
        // detach. (2026-07-02 Bug 2: deliberately NOT the events
        // channel's `receiver_count()` — internal daemon observers
        // hold receivers there and inflated the old count forever.)
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
/// read (`k2 read <workspace> [--agent <name>]`).
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

    #[test]
    fn terminal_sandbox_param_values() {
        let p = |v: &str| HashMap::from([("sandbox".to_string(), v.to_string())]);
        assert!(!terminal_sandbox_requested(&HashMap::new()), "no param: plain spawn");
        for no in ["false", "0", "no", "off", "", " FALSE "] {
            assert!(!terminal_sandbox_requested(&p(no)), "{no:?} is an explicit no");
        }
        for yes in ["true", "1", "yes", "TRUE", "please"] {
            assert!(terminal_sandbox_requested(&p(yes)), "{yes:?} asks for a sandbox");
        }
    }

    /// `k2 terminal spawn --sandbox` (`sandbox=true`) on a build that can't
    /// sandbox: 409 with the plain line and no session started under the
    /// name, on both routes. Before 0.46.17 the route ignored the flag and
    /// started the shell as you (200, a live session).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_spawn_sandbox_refuses_409_and_runs_nothing() {
        assert!(!crate::v2_spawn::can_sandbox(), "test premise: this build cannot sandbox");
        let _ = k2_core::db::init_for_tests();
        let home = crate::test_support::TempHome::new();
        let ws = home.path().join(format!("ws-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&ws).expect("workspace folder");
        let ws_str = dunce::canonicalize(&ws).expect("canonical folder").to_string_lossy().into_owned();
        // `sh`: a system shell the test spawn guard allows, so a spawn that
        // wrongly goes ahead really starts (and shows up in the map).
        let params_for = |agent: &str, sandbox: &str| {
            HashMap::from([
                ("agent".to_string(), agent.to_string()),
                ("command".to_string(), "sh".to_string()),
                ("cwd".to_string(), ws_str.clone()),
                ("sandbox".to_string(), sandbox.to_string()),
            ])
        };
        for (route, background) in [("spawn", false), ("spawn-background", true)] {
            let agent = format!("agent-a-sandbox-{route}-{}", uuid::Uuid::new_v4().simple());
            let params = params_for(&agent, "true");
            let resp = if background {
                handle_terminal_spawn_background(&params, &ws_str)
            } else {
                handle_terminal_spawn(&params, &ws_str)
            };
            let leaked = crate::v2_session_map::unregister(&agent);
            if let Some(ref s) = leaked {
                s.kill();
            }
            assert_eq!(resp.status, "409 Conflict", "{route}: body={}", resp.body);
            let v: serde_json::Value = serde_json::from_str(&resp.body).expect("409 body is JSON");
            assert_eq!(
                v["error"],
                serde_json::json!("this computer can't run a sandboxed session. Run it without --sandbox, knowing it runs as you."),
                "{route}: body={}",
                resp.body
            );
            assert_eq!(v["code"], serde_json::json!("sandbox_unavailable"));
            assert!(leaked.is_none(), "{route}: a refused sandbox request must start no session");
        }

        // `--no-sandbox` (`sandbox=false`) is the plain spawn, and the reply
        // says plainly that no sandbox is in force.
        let agent = format!("agent-a-plain-{}", uuid::Uuid::new_v4().simple());
        let resp = handle_terminal_spawn(&params_for(&agent, "false"), &ws_str);
        let live = crate::v2_session_map::unregister(&agent);
        if let Some(ref s) = live {
            s.kill();
        }
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let v: serde_json::Value = serde_json::from_str(&resp.body).expect("json");
        assert_eq!(v["sandbox"], serde_json::json!("none"), "body={}", resp.body);
        assert!(live.is_some(), "the plain spawn started a session");
    }

    /// One `k2 terminal spawn --agent <a> --command <command>` in `ws`.
    /// Dropping it ends the session (`unregister` kills the child, so it
    /// waits until the test has seen what the command did).
    #[cfg(unix)]
    struct Spawned {
        resp: CliResponse,
        agent: String,
    }

    #[cfg(unix)]
    impl Spawned {
        fn session(&self) -> Option<std::sync::Arc<k2_core::terminal::daemon_pty::DaemonPtySession>> {
            crate::v2_session_map::lookup_by_agent_name(&self.agent)
        }
    }

    #[cfg(unix)]
    impl Drop for Spawned {
        fn drop(&mut self) {
            crate::v2_session_map::unregister(&self.agent);
        }
    }

    #[cfg(unix)]
    fn spawn_command(ws: &str, command: &str) -> Spawned {
        let agent = format!("agent-cmdline-{}", uuid::Uuid::new_v4().simple());
        let params = HashMap::from([
            ("agent".to_string(), agent.clone()),
            ("command".to_string(), command.to_string()),
            ("cwd".to_string(), ws.to_string()),
        ]);
        Spawned { resp: handle_terminal_spawn(&params, ws), agent }
    }

    #[cfg(unix)]
    fn wait_for_file(p: &std::path::Path) -> String {
        let t = std::time::Instant::now();
        loop {
            if let Ok(s) = std::fs::read_to_string(p) {
                if !s.is_empty() {
                    return s;
                }
            }
            assert!(t.elapsed() < std::time::Duration::from_secs(20), "{} never written", p.display());
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    /// 0.46.18: `k2 terminal spawn --command` runs a full command line
    /// (arguments, a pipe, quotes) in the login shell; one word (a path)
    /// still starts as the program itself. Before, the whole string was
    /// argv[0] and every command with an argument failed with ENOENT
    /// (`v2 spawn failed: ptyd spawn: enoent` on the ptyd path). This is the
    /// in-process path (`terminal.ptyd` off); k2-core's `ptyd_backend`
    /// runs the same command lines through k2-ptyd. An agent CLI hidden in
    /// a command line is still refused by the test spawn guard.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminal_spawn_runs_a_full_command_line_in_process() {
        let _ = k2_core::db::init_for_tests();
        let home = crate::test_support::TempHome::new();
        k2_core::app_settings::update(serde_json::json!({ "terminal": { "ptyd": "off" } })).expect("terminal.ptyd off");
        assert_eq!(
            k2_core::terminal::ptyd::Switch::resolve(),
            k2_core::terminal::ptyd::Switch::Off,
            "in-process path (unset K2_PTYD for this test)"
        );
        let ws = home.path().join(format!("ws-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&ws).expect("workspace folder");
        let ws = dunce::canonicalize(&ws).expect("canonical folder");
        let ws_str = ws.to_string_lossy().into_owned();
        let m = |name: &str| ws.join(name);
        let ok = |sp: &Spawned, what: &str| {
            assert_eq!(sp.resp.status, "200 OK", "{what}: body={}", sp.resp.body);
        };

        // The reported line: before, `ptyd spawn: enoent` / ENOENT.
        let sp = spawn_command(&ws_str, "/bin/echo hi");
        ok(&sp, "/bin/echo hi");
        drop(sp);

        // Arguments only.
        let sp = spawn_command(&ws_str, &format!("touch '{}'", m("multi").display()));
        ok(&sp, "multi-word");
        wait_for_file_to_exist(&m("multi"));
        drop(sp);

        // The session runs the login shell with `-lc <the command line>`.
        let sp = spawn_command(&ws_str, "sleep 30");
        ok(&sp, "sleep 30");
        let s = sp.session().expect("sleep is still running");
        let program = s.program.clone().expect("a program");
        assert!(std::path::Path::new(&program).is_absolute(), "the login shell by path: {program}");
        assert_eq!(s.args, vec!["-lc".to_string(), "sleep 30".to_string()]);
        drop(s);
        drop(sp);

        // A pipe.
        let sp = spawn_command(&ws_str, &format!("echo piped | tr a-z A-Z > '{}'", m("pipe").display()));
        ok(&sp, "pipe");
        assert_eq!(wait_for_file(&m("pipe")).trim(), "PIPED");
        drop(sp);

        // Quotes, both kinds.
        let sp = spawn_command(&ws_str, &format!("printf '%s|%s' \"a b\" 'c d' > '{}'", m("quotes").display()));
        ok(&sp, "quotes");
        assert_eq!(wait_for_file(&m("quotes")), "a b|c d");
        drop(sp);

        // One word, a path: started as the program itself, no shell.
        let script = m("one-word.sh");
        k2_core::test_env::write_executable(&script, &format!("#!/bin/sh\necho one > '{}'\nexec sleep 30\n", m("one").display()));
        let script_s = script.to_string_lossy().into_owned();
        {
            // The temp-HOME belt refuses any program outside the system bin
            // dirs; this script is ours.
            let _allow = k2_core::test_env::EnvVar::set(k2_core::terminal::agent_spawn_guard::ALLOW_REAL_ENV, "1");
            let sp = spawn_command(&ws_str, &script_s);
            ok(&sp, "single-word path");
            assert_eq!(wait_for_file(&m("one")).trim(), "one");
            let s = sp.session().expect("the script is still running");
            assert_eq!(s.program.as_deref(), Some(script_s.as_str()), "one word is the program");
            assert!(s.args.is_empty(), "no args: {:?}", s.args);
        }

        // An agent CLI inside a command line: still refused, nothing runs.
        let sp = spawn_command(&ws_str, "claude --foo");
        assert_ne!(sp.resp.status, "200 OK", "body={}", sp.resp.body);
        assert!(sp.resp.body.contains("refusing") && sp.resp.body.contains("claude"), "body={}", sp.resp.body);
        assert!(sp.session().is_none(), "a refused agent CLI starts no session");
    }

    #[cfg(unix)]
    fn wait_for_file_to_exist(p: &std::path::Path) {
        let t = std::time::Instant::now();
        while !p.exists() {
            assert!(t.elapsed() < std::time::Duration::from_secs(20), "{} never created", p.display());
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
}
