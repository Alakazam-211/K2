//! Workspace-targeted messaging — `k2so msg <workspace> "text" [--from <name>]`.
//!
//! **0.38.6 contract: deliver-or-loudly-fail.**
//!
//! `deliver_live` is the only delivery path used by `k2so msg`. It runs
//! a four-branch cascade against the recipient workspace's pinned chat
//! session, with an internal retry window so the chronic "first call
//! returns success but doesn't land" UX disappears:
//!
//!   1. `active_terminal_id` set + alive in v2_session_map → inject
//!      the message body into the live PTY.
//!   2. `active_terminal_id` null/stale, but an existing PTY's argv
//!      references `--resume <session_id>` for the workspace's saved
//!      claude session → inject into that PTY.
//!   3. Saved `session_id` (claude UUID) but no live PTY → spawn an
//!      interactive `claude --resume <session_id>`, then deliver.
//!   4. Neither → spawn an interactive `claude --session-id <new>`,
//!      then deliver.
//!
//! Every return path produces the canonical [`MsgResponse`] shape:
//!
//! ```json
//! { "success": bool, "target_session_id": uuid|null, "attempts": u8,
//!   "reason": str|null, "hint": str|null }
//! ```
//!
//! There are exactly four failure `reason` codes, each with a paired
//! `hint` that points the caller at their next step. No silent inbox
//! fallback. No two-shape response (pre-0.38.6 sometimes returned the
//! legacy `agent_hooks` shape `{injected_to_pty, published_to_bus, ...}`;
//! that path is retired for `msg`).
//!
//! The sender's identity is rendered as a `[from <name>] ` prefix on the
//! wire bytes the recipient's PTY receives. Auto-derived by the CLI from
//! the sender's workspace name when available; falls back to `external`
//! when the call originates outside any registered workspace.

use std::path::Path;
use std::time::Duration;

use k2_core::workspace::agent_identity::resolve_project_id;
use k2_core::db::schema::WorkspaceSession;
use k2_core::log_debug;
use k2_core::session::SessionId;
use serde::Serialize;

use crate::session_lookup;
use crate::spawn::{spawn_agent_session_v2_blocking, SpawnWorkspaceSessionRequest};

// ─────────────────────────────────────────────────────────────────────
// Failure reason classification
// ─────────────────────────────────────────────────────────────────────

/// Failure reasons surfaced by `deliver_live`. Each has a paired
/// `hint` describing the caller's next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MsgReason {
    /// `<workspace>` arg didn't resolve to any registered project.
    WorkspaceNotFound,
    /// Workspace exists but has no `AGENT.md` / mode is off.
    /// Spawn target cannot be determined.
    NoAgentMode,
    /// Spawn attempt failed (claude binary missing, FS write failure,
    /// etc.). Transient — eligible for retry.
    SpawnFailed,
    /// PTY existed but the child process exited during the write
    /// (race we used to mask as `injected_to_pty: true`). Transient.
    PtyDied,
}

impl MsgReason {
    fn code(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NoAgentMode => "no_agent_mode",
            Self::SpawnFailed => "spawn_failed",
            Self::PtyDied => "pty_died",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => {
                "Run `k2so connections list` to see available workspaces."
            }
            Self::NoAgentMode => {
                "Workspace has no agent. Use `k2so work send` to queue, or `k2so mode custom` to set up an agent."
            }
            Self::SpawnFailed => {
                "Spawn failed. Verify `claude` is on PATH for the daemon."
            }
            Self::PtyDied => {
                "Target session crashed during delivery. Check `~/.k2/daemon.stderr.log`."
            }
        }
    }

    /// Permanent reasons short-circuit the retry loop — waiting won't
    /// make them resolve. Surface to the caller immediately. Exposed
    /// for use by tests + future external classifier consumers; the
    /// hot path inside [`deliver_live`] does its own string match.
    #[allow(dead_code)]
    fn is_permanent(self) -> bool {
        matches!(self, Self::WorkspaceNotFound | Self::NoAgentMode)
    }
}

// ─────────────────────────────────────────────────────────────────────
// Canonical response shape
// ─────────────────────────────────────────────────────────────────────

/// Canonical response shape for every `k2so msg` call.
///
/// Serializes to the public JSON contract:
/// `{success, target_session_id, attempts, reason, hint}`.
/// Internal-only fields are `#[serde(skip)]` so they don't leak into
/// the CLI response.
#[derive(Debug, Clone, Serialize)]
pub struct MsgResponse {
    pub success: bool,
    pub target_session_id: Option<String>,
    pub attempts: u8,
    pub reason: Option<String>,
    pub hint: Option<String>,

    /// Internal: which cascade branch fired (debug + audit only).
    /// Not part of the canonical JSON.
    #[serde(skip)]
    pub branch: Option<String>,
}

impl MsgResponse {
    fn ok(target_session_id: String, branch: &str) -> Self {
        Self {
            success: true,
            target_session_id: Some(target_session_id),
            attempts: 1,
            reason: None,
            hint: None,
            branch: Some(branch.to_string()),
        }
    }

    fn fail(reason: MsgReason) -> Self {
        Self {
            success: false,
            target_session_id: None,
            attempts: 1,
            reason: Some(reason.code().to_string()),
            hint: Some(reason.hint().to_string()),
            branch: None,
        }
    }

}

// ─────────────────────────────────────────────────────────────────────
// Workspace token resolver
// ─────────────────────────────────────────────────────────────────────

/// Resolve a workspace token (name, absolute path, or UUID) to its
/// canonical filesystem path. Returns `None` when no `projects` row
/// matches.
pub fn resolve_workspace(token: &str) -> Option<String> {
    if token.is_empty() {
        return None;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();

    // Absolute path (cheapest case — user passes the cwd).
    if token.starts_with('/') {
        return conn
            .query_row(
                "SELECT path FROM projects WHERE path = ?1",
                rusqlite::params![token],
                |r| r.get::<_, String>(0),
            )
            .ok();
    }

    // UUID lookup. `projects.id` is a v4 UUID; cheap to detect by
    // length + dashes without pulling in the uuid crate for parsing.
    if token.len() == 36 && token.chars().filter(|c| *c == '-').count() == 4 {
        if let Ok(path) = conn.query_row(
            "SELECT path FROM projects WHERE id = ?1",
            rusqlite::params![token],
            |r| r.get::<_, String>(0),
        ) {
            return Some(path);
        }
    }

    // Name match. Workspace names are short and usually unique within
    // the user's set; if multiple workspaces share a name we return
    // the first by insertion order (most users won't hit this).
    if let Ok(path) = conn.query_row(
        "SELECT path FROM projects WHERE name = ?1 ORDER BY rowid LIMIT 1",
        rusqlite::params![token],
        |r| r.get::<_, String>(0),
    ) {
        return Some(path);
    }

    // 0.39.45 (#33): case-insensitive fallback. Operators (and LLM
    // agents especially) infer plausible casing from context — `appa`
    // for a workspace registered as `Appa` — and used to bounce with
    // workspace_not_found on wiring that was actually fine. Exact-case
    // still wins above when two projects differ only by case.
    conn.query_row(
        "SELECT path FROM projects WHERE name = ?1 COLLATE NOCASE ORDER BY rowid LIMIT 1",
        rusqlite::params![token],
        |r| r.get::<_, String>(0),
    )
    .ok()
}

// ─────────────────────────────────────────────────────────────────────
// Bytes formatter — `[from <name>] <text>` prefix
// ─────────────────────────────────────────────────────────────────────

/// Format the wire bytes that go into the recipient's PTY. Compact
/// prefix on the first line; multi-line text keeps the prefix on
/// line 1 only, subsequent lines flow unprefixed.
///
/// Examples:
/// - `format_message("scout_v3", "hello", "")` → `"[from scout_v3] hello"`
/// - `format_message("scout_v3", "line1\nline2", "")` →
///   `"[from scout_v3] line1\nline2"`
/// - `format_message("scout_v3", "hi", "/loop")` →
///   `"/loop [from scout_v3] hi"` — `command` (trimmed) is prepended at
///   the VERY FRONT, before the `[from <name>]` prefix. Used to deliver
///   slash-commands (e.g. `/loop`, `/goal`) to the recipient's TUI.
/// - Empty `command` (the default) leaves the message unchanged.
/// - Empty `from` defaults to `external` (defense in depth; CLI should
///   already do this auto-derive, but we never let an empty prefix
///   reach the recipient).
pub fn format_message(from: &str, text: &str, command: &str) -> String {
    let sender = if from.trim().is_empty() {
        "external"
    } else {
        from
    };
    let command = command.trim();
    if command.is_empty() {
        format!("[from {sender}] {text}")
    } else {
        format!("{command} [from {sender}] {text}")
    }
}

// ─────────────────────────────────────────────────────────────────────
// Public entry — `deliver_live` (retry-wrapped)
// ─────────────────────────────────────────────────────────────────────
//
// Phase 2.1 wrap-up (0.39.0f): the pre-0.38.6 `deliver_to_inbox` helper
// (and its dependency on `k2_core::agents::commands::workspace_inbox_create`,
// which wrote to the retired `.k2so/work/inbox/` layout) was removed
// here. `msg` is strictly live-or-fail; new inbox-delivery callers
// should compose against `k2_core::inbox::compose` directly so they
// land in the canonical `.k2so/inbox/` location the renderer reads.

const MAX_ATTEMPTS: u8 = 3;
const BACKOFF_MS: [u64; 2] = [200, 400];

/// Deliver `text` live to `workspace_token`'s pinned agent session.
///
/// Wraps the four-branch cascade ([`attempt_delivery`]) in a retry
/// loop. The empirical observation across every C3PO failure ticket
/// is that re-fire almost always succeeds — the chronic re-fire UX is
/// driven by a transient race between the first call's spawn and the
/// second call seeing `active_terminal_id` populated. The retry window
/// absorbs that race without surfacing complexity to callers.
///
/// - `MAX_ATTEMPTS = 3` (1 initial + 2 retries)
/// - Backoffs: 200ms, 400ms (≤ 600ms worst case)
/// - Permanent reasons (`workspace_not_found`, `no_agent_mode`)
///   short-circuit immediately.
/// - Transient reasons (`spawn_failed`, `pty_died`) get retried.
///
/// `from` is rendered into the recipient's PTY as a `[from <name>] `
/// prefix. The CLI is expected to auto-derive this from the sender's
/// workspace (CWD / `K2SO_PROJECT_PATH`); when empty, the daemon
/// substitutes `external` so the recipient always sees a sender ID.
///
/// `command` (when non-empty) is a slash-command (e.g. `/loop`) that is
/// prepended at the VERY FRONT of the payload, before the `[from ...]`
/// prefix. Empty = unchanged delivery (the default).
pub fn deliver_live(
    workspace_token: &str,
    text: &str,
    from: &str,
    command: &str,
) -> MsgResponse {
    // Resolve workspace once. WorkspaceNotFound is permanent; surface
    // immediately without entering the retry loop. 0.39.45 (#33): the
    // hint carries a did-you-mean suggestion when a close name exists.
    let project_path = match resolve_workspace(workspace_token) {
        Some(p) => p,
        None => {
            let mut resp = MsgResponse::fail(MsgReason::WorkspaceNotFound);
            let suggestion = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::connections::suggest_project_name(&conn, workspace_token)
            };
            if let Some(s) = suggestion {
                resp.hint = Some(format!(
                    "Unknown workspace '{workspace_token}' — did you mean '{s}'? Run `k2so connections list` to see available workspaces."
                ));
            }
            return resp;
        }
    };

    let mut last: Option<MsgResponse> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let mut result = attempt_delivery(&project_path, text, from, command);
        result.attempts = attempt;

        if result.success {
            return result;
        }

        // Permanent reasons short-circuit — waiting won't fix them.
        if let Some(reason) = result.reason.as_deref() {
            let permanent = matches!(reason, "workspace_not_found" | "no_agent_mode");
            if permanent {
                return result;
            }
        }

        last = Some(result);

        if attempt < MAX_ATTEMPTS {
            std::thread::sleep(Duration::from_millis(BACKOFF_MS[(attempt - 1) as usize]));
        }
    }

    // Exhausted retries — return the last failure, with attempts=MAX.
    last.unwrap_or_else(|| MsgResponse::fail(MsgReason::SpawnFailed))
}

// ─────────────────────────────────────────────────────────────────────
// Inner — single delivery attempt (the cascade)
// ─────────────────────────────────────────────────────────────────────

/// One delivery attempt. Returns `MsgResponse` with `attempts: 1`;
/// the [`deliver_live`] retry wrapper rewrites it on retry.
fn attempt_delivery(project_path: &str, text: &str, from: &str, command: &str) -> MsgResponse {
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match resolve_project_id(&conn, project_path) {
            Some(p) => p,
            // Race: project removed between resolve_workspace + here.
            None => return MsgResponse::fail(MsgReason::WorkspaceNotFound),
        }
    };

    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::get(&conn, &project_id).ok().flatten()
    };

    let saved_session = row
        .as_ref()
        .and_then(|r| r.session_id.clone())
        .filter(|s| !s.is_empty());
    let saved_terminal = row
        .as_ref()
        .and_then(|r| r.active_terminal_id.clone())
        .filter(|s| !s.is_empty());

    // Branch 1: active_terminal_id alive → inject.
    if let Some(active_tid) = saved_terminal.as_deref() {
        if let Some(sid) = SessionId::parse(active_tid) {
            if let Some(live) = session_lookup::lookup_by_session_id(&sid) {
                return inject_live(&live, text, from, command, "active_terminal_id", &project_id);
            }
        }
        // Stale stamp — clear so downstream branches don't re-trip on it.
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::clear_active_terminal_id(&conn, &project_id);
    }

    // Branch 1b: argv-scan fallback for the cold-start case (any live
    // PTY running --resume <saved_session>).
    if let Some(claude_sid) = saved_session.as_deref() {
        for (_n, live) in session_lookup::snapshot_all() {
            let args = live.args();
            let mut i = 0;
            let mut found = false;
            while i + 1 < args.len() {
                if (args[i] == "--session-id" || args[i] == "--resume")
                    && args[i + 1] == claude_sid
                {
                    found = true;
                    break;
                }
                i += 1;
            }
            if found {
                return inject_live(&live, text, from, command, "argv_scan", &project_id);
            }
        }
    }

    // Branch 2: saved session, no live PTY → resume + fire.
    if let Some(claude_sid) = saved_session.as_deref() {
        return resume_and_fire(project_path, &project_id, claude_sid, text, from, command);
    }

    // Branch 3: fresh fire — no saved session at all.
    fresh_fire(project_path, &project_id, text, from, command)
}

// ─────────────────────────────────────────────────────────────────────
// Inject + submit (0.39.45 reliability fixes; 0.40.3 honest success-check)
// ─────────────────────────────────────────────────────────────────────
//
// The pre-0.39.45 injection was `write(text)` → sleep 150ms → `write(\r)`
// → return success. Under host CPU oversubscription the recipient TUI
// reads text + CR in one coalesced burst; without paste framing, its
// input widget absorbs the CR as a literal newline instead of a submit
// keystroke — the message sat un-submitted until a human pressed Enter
// (GH #38; surfaced as #36 submit stall / #34 receive buffering / #30
// stalled pings).
//
// 0.39.45 fixed RELIABILITY with two changes, both kept here:
//   1. PASTE FRAMING — when the recipient has bracketed-paste mode on
//      (claude/cursor TUIs enable it at startup), wrap the payload in
//      explicit `ESC[200~ … ESC[201~` markers. The trailing CR is then
//      unambiguously a submit keystroke even when the child reads the
//      whole burst in one `read()`.
//   2. EXTRA ENTER — under latency the first CR can land before the TUI
//      has finished ingesting the paste; send a second CR after a short
//      settle. An Enter on an already-empty input box is a no-op, so the
//      insurance keystroke is harmless when the first already submitted.
//
// 0.39.45 ALSO added a grid-scrape "did the input box clear?" oracle that
// reported `no_submit` when it couldn't confirm. With paste framing in
// place, sends now land reliably — but the oracle false-negatived: right
// after a submit, Claude echoes the message into the transcript with the
// SAME `"> "` prefix, and before the fresh empty input box repaints, that
// echo is the LAST `"> "` row on screen, so the scraper reads the tail as
// "still in the input box" and reports `no_submit` on a message that DID
// deliver (GH Alakazam-211/K2 #1). 0.40.3 retires the oracle and returns
// to the original, accurate success-check: a payload + submit CR written
// into a still-alive child IS a successful send. PTY death is still a
// loud `pty_died`.

/// Inject `payload` + submit into `live`, with paste framing and an
/// insurance Enter for latency. `Ok(())` = delivered to a live child;
/// `Err(())` = the child died mid-injection (caller reports `pty_died`).
fn inject_and_submit(
    live: &session_lookup::LiveSession,
    payload: &str,
) -> Result<(), ()> {
    let body: Vec<u8> = if live.bracketed_paste_active() {
        let mut b = Vec::with_capacity(payload.len() + 12);
        b.extend_from_slice(b"\x1b[200~");
        b.extend_from_slice(payload.as_bytes());
        b.extend_from_slice(b"\x1b[201~");
        b
    } else {
        payload.as_bytes().to_vec()
    };
    if live.write(&body).is_err() {
        return Err(());
    }
    // Let the TUI ingest + render the body before the submit keystroke.
    std::thread::sleep(Duration::from_millis(150));

    // Submit. The second CR after a settle is the latency insurance: if
    // the first landed before the paste finished ingesting, this one
    // submits it; if the first already submitted, this hits an empty
    // input box and no-ops.
    if live.write(b"\r").is_err() {
        return Err(());
    }
    std::thread::sleep(Duration::from_millis(250));
    if !live.is_child_alive() {
        return Err(());
    }
    let _ = live.write(b"\r");
    std::thread::sleep(Duration::from_millis(120));
    if !live.is_child_alive() {
        return Err(());
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Branch implementations
// ─────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn inject_live(
    live: &session_lookup::LiveSession,
    text: &str,
    from: &str,
    command: &str,
    branch: &str,
    project_id: &str,
) -> MsgResponse {
    let payload = format_message(from, text, command);
    if inject_and_submit(live, &payload).is_err() {
        log_debug!("[msg/inject_live] PTY died during injection — pty_died");
        return MsgResponse::fail(MsgReason::PtyDied);
    }

    let target_id = live.session_id().to_string();

    // Re-stamp `active_terminal_id` so subsequent calls fast-path
    // through Branch 1 directly. Idempotent if it already pointed here.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
    }

    MsgResponse::ok(target_id, branch)
}

#[allow(clippy::too_many_arguments)]
fn resume_and_fire(
    project_path: &str,
    project_id: &str,
    claude_sid: &str,
    text: &str,
    from: &str,
    command: &str,
) -> MsgResponse {
    let agent_name = match k2_core::workspace::agent_identity::find_primary_agent(project_path) {
        Some(n) => n,
        None => return MsgResponse::fail(MsgReason::NoAgentMode),
    };
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--resume".to_string(),
        claude_sid.to_string(),
    ];
    spawn_and_inject(
        project_path,
        project_id,
        &agent_name,
        args,
        text,
        from,
        command,
        "resume_and_fire",
        Some(claude_sid),
    )
}

fn fresh_fire(
    project_path: &str,
    project_id: &str,
    text: &str,
    from: &str,
    command: &str,
) -> MsgResponse {
    let agent_name = match k2_core::workspace::agent_identity::find_primary_agent(project_path) {
        Some(n) => n,
        None => return MsgResponse::fail(MsgReason::NoAgentMode),
    };
    let new_sid = uuid::Uuid::new_v4().to_string();
    let args = vec![
        "--dangerously-skip-permissions".to_string(),
        "--session-id".to_string(),
        new_sid.clone(),
    ];
    spawn_and_inject(
        project_path,
        project_id,
        &agent_name,
        args,
        text,
        from,
        command,
        "fresh_fire",
        Some(&new_sid),
    )
}

#[allow(clippy::too_many_arguments)]
fn spawn_and_inject(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    args: Vec<String>,
    text: &str,
    from: &str,
    command: &str,
    branch: &str,
    claude_session_id: Option<&str>,
) -> MsgResponse {
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: Some(project_id.to_string()),
        cwd: project_path.to_string(),
        command: Some("claude".to_string()),
        args: Some(args),
        cols: 120,
        rows: 38,
        canonical_key: None,
    }) {
        Ok(o) => o,
        Err(e) => {
            log_debug!("[msg/spawn_and_inject] spawn failed: {e}");
            return MsgResponse::fail(MsgReason::SpawnFailed);
        }
    };
    let target_id = outcome.session_id.to_string();

    // Upsert the workspace_sessions row + stamp session_id. Pre-0.37.5
    // history: see git log for `0.37.5 Phase D` if you want the
    // archaeology.
    let canonical_terminal_id = format!("agent-chat:{project_id}");
    let _ = k2_core::workspace::session::k2so_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(canonical_terminal_id),
        Some("system".to_string()),
    );
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
        if let Some(sid) = claude_session_id {
            let _ = WorkspaceSession::update_session_id(&conn, project_id, sid);
        }
    }

    log_debug!(
        "[daemon/workspace-msg] {} session={} agent={} from={}",
        branch,
        target_id,
        agent_name,
        from
    );

    // Two-phase write — wait for claude TUI to draw before sending the
    // body. Apply the `[from <name>] ` prefix to the bytes the
    // recipient's PTY receives. The body goes through the same injector
    // as live delivery — paste framing + insurance Enter.
    let session = session_lookup::lookup_by_session_id(&outcome.session_id);
    if let Some(live) = session {
        let payload = format_message(from, text, command);
        let log_sid = target_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            if inject_and_submit(&live, &payload).is_err() {
                log_debug!(
                    "[daemon/workspace-msg] PTY died during post-spawn inject for session={}",
                    log_sid
                );
            }
        });
    } else {
        log_debug!(
            "[daemon/workspace-msg] post-spawn lookup miss for session={} — body not delivered",
            target_id
        );
    }

    let _ = Path::new(project_path);
    MsgResponse::ok(target_id, branch)
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Bytes formatter ──────────────────────────────────────────────

    #[test]
    fn format_message_compact_prefix() {
        assert_eq!(
            format_message("scout_v3", "hello world", ""),
            "[from scout_v3] hello world"
        );
    }

    #[test]
    fn format_message_multiline_first_line_only() {
        let out = format_message("scout_v3", "First line\nSecond line\nThird", "");
        assert_eq!(out, "[from scout_v3] First line\nSecond line\nThird");
    }

    #[test]
    fn format_message_empty_from_falls_back_to_external() {
        assert_eq!(format_message("", "hello", ""), "[from external] hello");
        assert_eq!(format_message("   ", "hello", ""), "[from external] hello");
    }

    #[test]
    fn format_message_custom_sender_passes_through() {
        assert_eq!(
            format_message("sms-bridge", "ping", ""),
            "[from sms-bridge] ping"
        );
    }

    // ── Command prefix (0.39.25 `--command`) ─────────────────────────

    #[test]
    fn format_message_command_prepends_before_from_prefix() {
        // The slash-command lands at the VERY FRONT, before `[from ...]`.
        assert_eq!(
            format_message("scout_v3", "hi", "/loop"),
            "/loop [from scout_v3] hi"
        );
    }

    #[test]
    fn format_message_empty_command_is_unchanged() {
        // Empty command → behavior identical to the pre-0.39.25 path.
        assert_eq!(
            format_message("scout_v3", "hi", ""),
            "[from scout_v3] hi"
        );
    }

    #[test]
    fn format_message_command_is_trimmed() {
        // Surrounding whitespace on the command is stripped; the body
        // and `[from ...]` prefix are untouched.
        assert_eq!(
            format_message("scout_v3", "hi", "  /loop  "),
            "/loop [from scout_v3] hi"
        );
    }

    #[test]
    fn format_message_whitespace_only_command_falls_back_to_unchanged() {
        // A command that trims to empty must NOT inject a stray space.
        assert_eq!(
            format_message("scout_v3", "hi", "   "),
            "[from scout_v3] hi"
        );
    }

    #[test]
    fn format_message_command_with_empty_from_uses_external() {
        // command + empty from → command still leads, sender = external.
        assert_eq!(
            format_message("", "hi", "/goal"),
            "/goal [from external] hi"
        );
    }

    // ── MsgReason classification ─────────────────────────────────────

    #[test]
    fn workspace_not_found_is_permanent() {
        assert!(MsgReason::WorkspaceNotFound.is_permanent());
    }

    #[test]
    fn no_agent_mode_is_permanent() {
        assert!(MsgReason::NoAgentMode.is_permanent());
    }

    #[test]
    fn spawn_failed_is_transient() {
        assert!(!MsgReason::SpawnFailed.is_permanent());
    }

    #[test]
    fn pty_died_is_transient() {
        assert!(!MsgReason::PtyDied.is_permanent());
    }

    #[test]
    fn every_reason_has_a_nonempty_hint() {
        for reason in [
            MsgReason::WorkspaceNotFound,
            MsgReason::NoAgentMode,
            MsgReason::SpawnFailed,
            MsgReason::PtyDied,
        ] {
            assert!(
                !reason.hint().is_empty(),
                "{} hint must be non-empty",
                reason.code()
            );
        }
    }

    #[test]
    fn reason_codes_are_stable_strings() {
        // The CLI + downstream consumers match on these strings.
        // Lock the wire values so future refactors can't silently
        // change them.
        assert_eq!(MsgReason::WorkspaceNotFound.code(), "workspace_not_found");
        assert_eq!(MsgReason::NoAgentMode.code(), "no_agent_mode");
        assert_eq!(MsgReason::SpawnFailed.code(), "spawn_failed");
        assert_eq!(MsgReason::PtyDied.code(), "pty_died");
    }

    /// 0.40.3: the live-inject success-check reverted to alive-based —
    /// there is no `no_submit` reason anymore (the grid-scrape oracle
    /// that false-negatived delivered messages was retired). Lock the
    /// reason set so it can't silently creep back.
    #[test]
    fn no_submit_reason_is_retired() {
        for code in [
            MsgReason::WorkspaceNotFound.code(),
            MsgReason::NoAgentMode.code(),
            MsgReason::SpawnFailed.code(),
            MsgReason::PtyDied.code(),
        ] {
            assert_ne!(code, "no_submit", "no_submit must not return");
        }
    }

    // ── MsgResponse serialization (the canonical JSON contract) ─────

    #[test]
    fn ok_response_serializes_to_canonical_shape() {
        let r = MsgResponse::ok("abc-123".into(), "active_terminal_id");
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["success"], true);
        assert_eq!(json["target_session_id"], "abc-123");
        assert_eq!(json["attempts"], 1);
        assert!(json["reason"].is_null());
        assert!(json["hint"].is_null());
    }

    #[test]
    fn fail_response_serializes_to_canonical_shape() {
        let r = MsgResponse::fail(MsgReason::NoAgentMode);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["success"], false);
        assert!(json["target_session_id"].is_null());
        assert_eq!(json["reason"], "no_agent_mode");
        let hint = json["hint"].as_str().expect("hint must be present");
        assert!(
            hint.contains("k2so work send"),
            "no_agent_mode hint should point to work send"
        );
        assert_eq!(json["attempts"], 1);
    }

    #[test]
    fn legacy_field_names_never_appear_in_canonical_response() {
        // These shipped pre-0.38.6 and caused the chronic re-fire UX.
        // The new canonical response MUST NOT include any of them —
        // they confuse callers who matched on the old keys.
        let cases = vec![
            MsgResponse::ok("sid".into(), "fresh_spawn"),
            MsgResponse::fail(MsgReason::WorkspaceNotFound),
            MsgResponse::fail(MsgReason::SpawnFailed),
            MsgResponse::fail(MsgReason::PtyDied),
        ];
        let legacy_fields = [
            "branch",
            "delivery",
            "targetSessionId",
            "error",
            "injected_to_pty",
            "published_to_bus",
            "woke_offline_target",
            "activity_feed_row_id",
            "inbox_path",
            "agent",
            "result",
        ];
        for r in &cases {
            let json = serde_json::to_value(r).unwrap();
            for legacy in &legacy_fields {
                assert!(
                    json.get(*legacy).is_none(),
                    "legacy field `{}` leaked into MsgResponse JSON: {:?}",
                    legacy,
                    json
                );
            }
        }
    }

    #[test]
    fn ok_response_carries_internal_branch_for_audit() {
        // The `branch` field is NOT serialized to JSON, but internal
        // Rust callers (heartbeat audit log, debug printouts) need it.
        let r = MsgResponse::ok("sid".into(), "fresh_spawn");
        assert_eq!(r.branch.as_deref(), Some("fresh_spawn"));
    }

    // ── deliver_live behavior ────────────────────────────────────────

    #[test]
    fn deliver_live_unknown_workspace_returns_workspace_not_found() {
        // No DB setup, no project rows — resolver misses, we return
        // permanent failure immediately (no retry loop).
        let r = deliver_live("definitely-not-a-real-workspace-name", "hi", "sender", "");
        assert!(!r.success);
        assert_eq!(r.reason.as_deref(), Some("workspace_not_found"));
        // Permanent reasons short-circuit — no retries.
        assert_eq!(r.attempts, 1);
        assert!(r.target_session_id.is_none());
        assert!(r
            .hint
            .as_deref()
            .is_some_and(|h| h.contains("connections list")));
    }

    #[test]
    fn deliver_live_empty_workspace_token_returns_workspace_not_found() {
        // Empty token must not match every row (resolve_workspace
        // short-circuits to None).
        let r = deliver_live("", "hi", "sender", "");
        assert_eq!(r.reason.as_deref(), Some("workspace_not_found"));
        assert_eq!(r.attempts, 1);
    }
}
