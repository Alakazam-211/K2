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
    /// 0.39.45 (GH #38/#36/#34/#30): the payload was injected into the
    /// recipient's PTY, but submission could NOT be confirmed — the
    /// text still sat in the recipient TUI's input box after the
    /// submit CR (re-sent several times with backoff). Pre-0.39.45
    /// this surfaced as `success: true` and the message silently never
    /// delivered. PERMANENT for the retry loop: re-running the whole
    /// cascade would re-inject the full text and duplicate it in the
    /// recipient's input box.
    NoSubmit,
}

impl MsgReason {
    fn code(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NoAgentMode => "no_agent_mode",
            Self::SpawnFailed => "spawn_failed",
            Self::PtyDied => "pty_died",
            Self::NoSubmit => "no_submit",
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
                "Target session crashed during delivery. Check `~/.k2so/daemon.stderr.log`."
            }
            Self::NoSubmit => {
                "Message text was injected but Enter was not confirmed — it may sit un-submitted in the recipient's input box. Do NOT resend the full text; send a short follow-up ping or check the recipient session."
            }
        }
    }

    /// Permanent reasons short-circuit the retry loop — waiting won't
    /// make them resolve. Surface to the caller immediately. Exposed
    /// for use by tests + future external classifier consumers; the
    /// hot path inside [`deliver_live`] does its own string match.
    #[allow(dead_code)]
    fn is_permanent(self) -> bool {
        matches!(
            self,
            Self::WorkspaceNotFound | Self::NoAgentMode | Self::NoSubmit
        )
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

    /// Failure that still identifies the target session — used by
    /// `no_submit` (GH #38) so the sender can locate the session whose
    /// input box may hold the stranded payload.
    fn fail_with_target(reason: MsgReason, target_session_id: String) -> Self {
        Self {
            target_session_id: Some(target_session_id),
            ..Self::fail(reason)
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
        // `no_submit` (0.39.45, GH #38) is permanent BY DESIGN: the
        // payload is already sitting in the recipient's input box, so
        // re-running the cascade would inject the full text a second
        // time and duplicate it.
        if let Some(reason) = result.reason.as_deref() {
            let permanent = matches!(
                reason,
                "workspace_not_found" | "no_agent_mode" | "no_submit"
            );
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
// Verified submit (0.39.45, GH #38/#36/#34/#30)
// ─────────────────────────────────────────────────────────────────────
//
// The pre-0.39.45 injection was `write(text)` → sleep 150ms → `write(\r)`
// → return success, with ZERO verification. Under host CPU
// oversubscription the recipient TUI reads text + CR in one coalesced
// burst; without paste framing, its input widget absorbs the CR as a
// literal newline instead of a submit keystroke — the message sits
// un-submitted in the input box until a human presses Enter (GH #38's
// root-cause writeup; surfaces as #36 submit stall / #34 receive-side
// buffering / #30 stalled pings).
//
// Two-layer fix:
//   1. STRUCTURAL — when the recipient has bracketed-paste mode on
//      (claude/cursor TUIs enable it at startup), wrap the payload in
//      explicit `ESC[200~ … ESC[201~` markers. The trailing CR is then
//      unambiguously a keystroke even when the child reads the whole
//      burst in one `read()`.
//   2. VERIFY-AND-RESUBMIT — after the CR, read the recipient's visible
//      grid and check whether the payload tail still sits in the INPUT
//      REGION (the rows from the LAST `"> "` prompt row to the bottom of
//      the screen — claude's input box is the last prompt on screen;
//      transcript echoes of submitted messages sit ABOVE it). While it
//      does, re-send the CR with backoff. An extra Enter on an already-
//      empty input box is a no-op, so a false re-send is harmless.
//
// When the tail never clears, the caller reports `no_submit` (loud)
// instead of the pre-0.39.45 `success: true` (silent loss).

const SUBMIT_SETTLE_MS: [u64; 4] = [250, 450, 800, 1200];

/// Strip everything but ASCII alphanumerics. Grid rows interleave the
/// payload with box-drawing borders, wrap points, and padding; the
/// payload may wrap anywhere. Comparing alnum-only streams makes the
/// tail match immune to all of that.
fn normalize_for_match(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

/// The marker we search for: the last ≤32 alphanumeric chars of the
/// payload. Empty when the payload has no alphanumerics (verification
/// is skipped — nothing reliable to match on).
fn payload_tail_marker(payload: &str) -> String {
    let norm = normalize_for_match(payload);
    // Safe byte slicing: `normalize_for_match` output is pure ASCII.
    let start = norm.len().saturating_sub(32);
    norm[start..].to_string()
}

/// True when the recipient's input region still shows `tail`.
///
/// Region = rows from the LAST row containing the `"> "` input prompt
/// through the bottom of the screen. Claude Code renders submitted
/// messages with the same `"> "` prefix in the transcript, but those
/// sit ABOVE the input box — taking the LAST prompt row excludes them.
/// When no prompt row exists (non-claude TUI, mid-redraw), falls back
/// to the bottom 6 rows.
fn input_region_contains(rows: &[String], tail: &str) -> bool {
    if tail.is_empty() {
        return false;
    }
    let start = rows
        .iter()
        .rposition(|r| r.contains("> "))
        .unwrap_or_else(|| rows.len().saturating_sub(6));
    let joined: String = rows[start..]
        .iter()
        .map(|s| normalize_for_match(s))
        .collect();
    joined.contains(tail)
}

/// Whether the verified injection confirmed submission.
enum SubmitOutcome {
    /// The payload tail left the input region after a CR — observed
    /// submitted.
    Confirmed,
    /// All CR attempts exhausted with the tail still in the input
    /// region, or the payload had nothing to verify against.
    Unverified,
}

/// Inject `payload` + submit CR into `live`, with paste framing and
/// verify-and-resubmit. `Err(())` = the child died mid-injection.
fn inject_payload_verified(
    live: &session_lookup::LiveSession,
    payload: &str,
) -> Result<SubmitOutcome, ()> {
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

    let tail = payload_tail_marker(payload);
    for (i, settle) in SUBMIT_SETTLE_MS.iter().enumerate() {
        if live.write(b"\r").is_err() {
            return Err(());
        }
        std::thread::sleep(Duration::from_millis(*settle));
        if !live.is_child_alive() {
            return Err(());
        }
        if tail.is_empty() {
            // Nothing to verify against — bytes are delivered, paste
            // framing makes the CR unambiguous; report unverified.
            return Ok(SubmitOutcome::Unverified);
        }
        let rows = live.visible_text_rows();
        if !input_region_contains(&rows, &tail) {
            if i > 0 {
                log_debug!(
                    "[msg/verify] submit confirmed after {} CR attempt(s)",
                    i + 1
                );
            }
            return Ok(SubmitOutcome::Confirmed);
        }
        log_debug!(
            "[msg/verify] payload tail still in input region after CR #{} — re-sending Enter",
            i + 1
        );
    }
    Ok(SubmitOutcome::Unverified)
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
    let outcome = match inject_payload_verified(live, &payload) {
        Ok(o) => o,
        Err(()) => {
            log_debug!("[msg/inject_live] PTY died during injection — pty_died");
            return MsgResponse::fail(MsgReason::PtyDied);
        }
    };

    let target_id = live.session_id().to_string();

    // Re-stamp `active_terminal_id` so subsequent calls fast-path
    // through Branch 1 directly. Idempotent if it already pointed here.
    // Stamped on BOTH outcomes — the session is live either way.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
    }

    match outcome {
        SubmitOutcome::Confirmed => MsgResponse::ok(target_id, branch),
        SubmitOutcome::Unverified => {
            log_debug!(
                "[msg/inject_live] submission UNCONFIRMED for session={} — reporting no_submit",
                target_id
            );
            MsgResponse::fail_with_target(MsgReason::NoSubmit, target_id)
        }
    }
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
    // recipient's PTY receives. 0.39.45 (GH #38): the body now goes
    // through the same verified injector as live delivery — paste
    // framing + verify-and-resubmit — instead of fire-and-forget.
    let session = session_lookup::lookup_by_session_id(&outcome.session_id);
    if let Some(live) = session {
        let payload = format_message(from, text, command);
        let log_sid = target_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            match inject_payload_verified(&live, &payload) {
                Ok(SubmitOutcome::Confirmed) => {}
                Ok(SubmitOutcome::Unverified) => log_debug!(
                    "[daemon/workspace-msg] post-spawn inject UNCONFIRMED for session={} — payload may sit in the input box",
                    log_sid
                ),
                Err(()) => log_debug!(
                    "[daemon/workspace-msg] PTY died during post-spawn inject for session={}",
                    log_sid
                ),
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
    fn no_submit_is_permanent_so_the_payload_is_never_duplicated() {
        // GH #38: retrying the cascade after no_submit would re-inject
        // the full text on top of the copy already in the input box.
        assert!(MsgReason::NoSubmit.is_permanent());
    }

    #[test]
    fn every_reason_has_a_nonempty_hint() {
        for reason in [
            MsgReason::WorkspaceNotFound,
            MsgReason::NoAgentMode,
            MsgReason::SpawnFailed,
            MsgReason::PtyDied,
            MsgReason::NoSubmit,
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
        assert_eq!(MsgReason::NoSubmit.code(), "no_submit");
    }

    // ── Verified submit helpers (0.39.45, GH #38) ────────────────────

    #[test]
    fn tail_marker_takes_last_32_alnum_chars() {
        let payload = "[from scout_v3] the quick brown fox jumps over the lazy daemon";
        let tail = payload_tail_marker(payload);
        assert_eq!(tail.len(), 32);
        assert!(tail.ends_with("lazydaemon"));
        // Non-alnum (spaces, brackets, underscores) must be stripped.
        assert!(tail.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn tail_marker_short_payload_uses_everything() {
        assert_eq!(payload_tail_marker("[from x] hi"), "fromxhi");
    }

    #[test]
    fn tail_marker_empty_for_non_alnum_payload() {
        assert_eq!(payload_tail_marker("!!! ??? ..."), "");
    }

    /// Un-submitted state: the payload sits in the input box (the LAST
    /// "> " row), wrapped across the box's rows with borders. Must be
    /// detected so the CR gets re-sent.
    #[test]
    fn input_region_detects_unsubmitted_payload() {
        let rows: Vec<String> = vec![
            "● Earlier transcript content".into(),
            "".into(),
            "╭──────────────────────────────╮".into(),
            "│ > [from scout_v3] the quick │".into(),
            "│ brown fox jumps over the     │".into(),
            "│ lazy daemon                  │".into(),
            "╰──────────────────────────────╯".into(),
            "  ⏵⏵ bypass permissions on".into(),
        ];
        let tail = payload_tail_marker("[from scout_v3] the quick brown fox jumps over the lazy daemon");
        assert!(
            input_region_contains(&rows, &tail),
            "wrapped payload in the input box must be detected"
        );
    }

    /// Submitted state: the transcript echoes the message with the SAME
    /// "> " prefix ABOVE the (now empty) input box. The region must
    /// start at the LAST prompt row, excluding the echo — otherwise
    /// every successful submit would be misread as un-submitted.
    #[test]
    fn input_region_ignores_transcript_echo_after_submit() {
        let rows: Vec<String> = vec![
            "> [from scout_v3] the quick brown fox jumps over the lazy daemon".into(),
            "".into(),
            "● Working on it…".into(),
            "╭──────────────────────────────╮".into(),
            "│ >                            │".into(),
            "╰──────────────────────────────╯".into(),
            "  ⏵⏵ bypass permissions on".into(),
        ];
        let tail = payload_tail_marker("[from scout_v3] the quick brown fox jumps over the lazy daemon");
        assert!(
            !input_region_contains(&rows, &tail),
            "transcript echo above the input box must NOT read as un-submitted"
        );
    }

    /// No "> " prompt anywhere (non-claude TUI / mid-redraw): fall back
    /// to the bottom 6 rows.
    #[test]
    fn input_region_falls_back_to_bottom_rows_without_prompt() {
        let mut rows: Vec<String> = (0..20).map(|i| format!("row {i}")).collect();
        rows.push("stranded payload tail here".into());
        let tail = payload_tail_marker("stranded payload tail here");
        assert!(input_region_contains(&rows, &tail));

        // And when the tail is far ABOVE the bottom-6 window, it must
        // not match (it scrolled into history — not the input line).
        let mut rows2: Vec<String> = vec!["stranded payload tail here".into()];
        rows2.extend((0..20).map(|i| format!("row {i}")));
        assert!(!input_region_contains(&rows2, &tail));
    }

    #[test]
    fn input_region_empty_tail_never_matches() {
        let rows: Vec<String> = vec!["│ > anything".into()];
        assert!(!input_region_contains(&rows, ""));
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
