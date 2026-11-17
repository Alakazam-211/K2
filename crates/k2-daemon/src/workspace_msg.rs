//! Workspace-targeted messaging — `k2 msg <workspace> "text" [--from <name>]`.
//!
//! **0.38.6 contract: deliver-or-loudly-fail.**
//!
//! `deliver_live` is the only delivery path used by `k2 msg`. It runs
//! a four-branch cascade against the recipient workspace's pinned chat
//! session, with an internal retry window so the chronic "first call
//! returns success but doesn't land" UX disappears:
//!
//!   1. `active_terminal_id` set + alive in v2_session_map → inject
//!      the message body into the live PTY.
//!   2. `active_terminal_id` null/stale, but an existing PTY's argv
//!      references the workspace's saved session id (any supported
//!      resume grammar — `--resume`, `--session-id`, pi's `--session`,
//!      codex's `resume <id>`) → inject into that PTY.
//!   3. Saved `session_id` but no live PTY → wake: the canonical
//!      resume resolver (`resume_chat::resolve_resume_chat_args`)
//!      picks the agent (stored `workspace_sessions.harness` wins for
//!      an existing canonical session) + the provider's own resume
//!      argv, spawn interactively, then deliver. Slice 3b — pre-3b
//!      this hardcoded `claude --resume <id>` and woke the WRONG
//!      binary for non-claude workspaces (research dragon #3).
//!   4. Neither → fresh wake via the same resolver: premint providers
//!      (claude/grok) spawn with `--session-id <new>`, self-minting
//!      providers (pi/codex/gemini/cursor) spawn bare and adopt their
//!      discovered id post-hoc, unknown providers spawn bare.
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

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use parking_lot::Mutex as PlMutex;

use k2_core::db::schema::{Project, WorkspaceSession};
use k2_core::log_debug;
use k2_core::session::SessionId;
use k2_core::workspace::agent_identity::resolve_project_id;
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
    /// The workspace's pinned main chat has no conversation on disk. K2
    /// keeps the pin and does not start another chat (a person picks one:
    /// `k2 sessions main set <id>`). Permanent for this call.
    MainChatMissing,
    /// PTY existed but the child process exited during the write
    /// (race we used to mask as `injected_to_pty: true`). Transient.
    PtyDied,
    /// Composer 1a (D2/C2): the per-session injection lock did not free
    /// within the deadline — a prior send is still in flight, or the
    /// child is wedged. The waiter is released (it never holds the lock)
    /// and the caller is told truthfully instead of blocking forever.
    /// Transient — eligible for retry.
    PtyStalled,
    /// Issue #9: the workspace HAS a resolvable primary agent but its
    /// canonical session is asleep, and the caller did NOT request a wake
    /// (`k2 msg` without `--wake`). Distinct from [`Self::NoAgentMode`]
    /// (which is genuinely un-wakeable) — this peer CAN be woken, the
    /// caller just opted out. Permanent for THIS call (re-firing without
    /// `--wake` won't change anything); the remedy is `--wake` / `k2 talk`.
    DormantNoWake,
    /// Slice 5 hard-safety rule: the recipient grok pane is sitting on
    /// an OPEN permission gate whose default selection is
    /// "always-approve" — any submit keystroke we inject (the trailing
    /// CR, or the insurance CR) could grant blanket approval. Delivery
    /// is HELD (nothing written); transient like [`Self::PtyStalled`] —
    /// re-fire once a human resolves the gate.
    HitlGateOpen,
    /// prd-security-sweep-0464-v1 SS11: the recipient's folder has settings
    /// that run commands and waits for a person to trust it (a card is on
    /// Thread). K2 started no session; the wake is queued and runs once
    /// when the person answers "Trust it". Permanent for THIS call.
    FolderTrustPending,
    /// The owner chose "Don't trust" for the recipient's folder: K2 starts
    /// no AI tool there. Permanent.
    FolderTrustDenied,
}

impl MsgReason {
    fn code(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NoAgentMode => "no_agent_mode",
            Self::SpawnFailed => "spawn_failed",
            Self::MainChatMissing => "main_chat_missing",
            Self::PtyDied => "pty_died",
            Self::PtyStalled => "pty_stalled",
            Self::DormantNoWake => "dormant_no_wake",
            Self::HitlGateOpen => "hitl_gate_open",
            Self::FolderTrustPending => "folder_trust_pending",
            Self::FolderTrustDenied => "folder_trust_denied",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => {
                "Run `k2 connections list` to see available workspaces."
            }
            Self::NoAgentMode => {
                "Workspace has no agent. Use `k2 msg <workspace> --inbox-silent <file>` to queue, or `k2 settings --mode custom` to set up an agent."
            }
            Self::SpawnFailed => {
                "Spawn failed. Verify the workspace's agent CLI (e.g. `claude`) is on PATH for the daemon."
            }
            Self::MainChatMissing => {
                "This workspace's main chat can't be found on disk, so K2 started nothing. A person chooses a chat with `k2 sessions main set <id>` (list them with `k2 sessions --past`) or from the chat's menu."
            }
            Self::PtyDied => {
                "Target session crashed during delivery. Check `~/.k2/daemon.stderr.log`."
            }
            Self::PtyStalled => {
                "Target session's injection lock did not free within the deadline (a prior send is still in flight, or the child is wedged). Retry shortly."
            }
            Self::DormantNoWake => {
                "Peer's canonical session is asleep. Re-run with `--wake` (or use `k2 talk`, which auto-wakes) to wake it and deliver."
            }
            Self::HitlGateOpen => {
                "Recipient is sitting on an open permission gate (grok `⚠ Action Required` — its default answer is always-approve, so K2 held the message rather than risk auto-approving). Resolve the gate in the terminal, then re-send."
            }
            Self::FolderTrustPending => {
                "Waiting for the owner to trust the recipient's folder (it has settings that run commands when AI tools start). K2 starts the session and delivers this message when they answer Trust on Thread (or: k2 workspace trust <folder> --yes)."
            }
            Self::FolderTrustDenied => {
                "The owner chose \"Don't trust\" for the recipient's folder, so K2 starts no program there. They can change it in Settings → Workspaces → Folder trust."
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
            Self::WorkspaceNotFound
                | Self::NoAgentMode
                | Self::MainChatMissing
                | Self::DormantNoWake
                | Self::FolderTrustPending
                | Self::FolderTrustDenied
        )
    }
}

/// A wake spawn's error: the folder-trust gate's refusal (SS11) keeps its
/// own reason, so the sender is told the folder waits for a person.
fn spawn_fail_reason(e: &str) -> MsgReason {
    match k2_core::cli_folder_trust::gate::refusal_in_text(e) {
        Some(r) if r.error == k2_core::cli_folder_trust::gate::DENIED => MsgReason::FolderTrustDenied,
        Some(_) => MsgReason::FolderTrustPending,
        None => MsgReason::SpawnFailed,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Canonical response shape
// ─────────────────────────────────────────────────────────────────────

/// Canonical response shape for every `k2 msg` call.
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

    /// Issue #9: `true` when THIS call woke a dormant canonical session
    /// before delivering (so the caller can attribute the wall-clock cost
    /// and print "peer was dormant — woke (Xs) — delivering"). Only
    /// serialized when true so non-wake responses keep the lean canonical
    /// shape.
    #[serde(default, skip_serializing_if = "is_false")]
    pub woke: bool,

    /// Issue #9: wall-clock ms spent waking + waiting for the freshly
    /// spawned session to reach a ready (bracketed-paste) state before the
    /// body was injected. Present only when [`Self::woke`] is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_ms: Option<u64>,

    /// Internal: which cascade branch fired (debug + audit only).
    /// Not part of the canonical JSON.
    #[serde(skip)]
    pub branch: Option<String>,

    /// The sidecar address this delivery reached, when the caller used an
    /// older one (`k2/3` after a rename to `k2/reviewer`). Present only
    /// with [`Self::moved_from`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub addr: Option<String>,

    /// The address the caller sent, when the chat now answers elsewhere
    /// (a local rename only; TR17). The CLI prints
    /// `note: <movedFrom> is now <addr>`.
    #[serde(rename = "movedFrom", default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<String>,
}

/// serde `skip_serializing_if` helper — keep `woke: false` out of the wire.
fn is_false(b: &bool) -> bool {
    !*b
}

impl MsgResponse {
    fn ok(target_session_id: String, branch: &str) -> Self {
        Self {
            success: true,
            target_session_id: Some(target_session_id),
            attempts: 1,
            reason: None,
            hint: None,
            woke: false,
            wake_ms: None,
            branch: Some(branch.to_string()),
            addr: None,
            moved_from: None,
        }
    }

    /// Issue #9: a successful delivery that REQUIRED waking a dormant
    /// canonical session first. Carries the wake duration so the caller
    /// can attribute the latency.
    fn ok_woke(target_session_id: String, branch: &str, wake_ms: u64) -> Self {
        let mut r = Self::ok(target_session_id, branch);
        r.woke = true;
        r.wake_ms = Some(wake_ms);
        r
    }

    pub(crate) fn fail(reason: MsgReason) -> Self {
        Self {
            success: false,
            target_session_id: None,
            attempts: 1,
            reason: Some(reason.code().to_string()),
            hint: Some(reason.hint().to_string()),
            woke: false,
            wake_ms: None,
            branch: None,
            addr: None,
            moved_from: None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Workspace token resolver
// ─────────────────────────────────────────────────────────────────────

/// Resolve a workspace token (name, absolute path, or UUID) to its
/// canonical filesystem path. Returns `None` when no `projects` row
/// matches.
/// Inbox / tray compose: `sales/reviewer` is still the **sales** inbox
/// (workspace-shared). Strip the handle, then resolve as a workspace.
pub fn resolve_workspace_allowing_handle(token: &str) -> Option<String> {
    let ws = k2_core::workspace_session_handles::split_workspace_handle(token)
        .map(|(ws, _)| ws)
        .unwrap_or(token);
    resolve_workspace(ws)
}

pub fn resolve_workspace(token: &str) -> Option<String> {
    match resolve_workspace_detailed(token) {
        WorkspaceResolve::Found(path) => Some(path),
        WorkspaceResolve::Ambiguous { .. }
        | WorkspaceResolve::Retired { .. }
        | WorkspaceResolve::Miss => None,
    }
}

/// Local resolve with AMBIG surface (D19 / §9.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceResolve {
    Found(String),
    Ambiguous { handles: Vec<String> },
    /// A retired handle (0159): its workspace was removed and handles are
    /// never reused, so it names no workspace.
    Retired { handle: String },
    Miss,
}

pub fn resolve_workspace_detailed(token: &str) -> WorkspaceResolve {
    if token.is_empty() {
        return WorkspaceResolve::Miss;
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    match k2_core::workspace::handle::resolve_workspace_token(&conn, token) {
        k2_core::workspace::handle::WorkspaceTokenResolve::Found { path } => {
            WorkspaceResolve::Found(path)
        }
        k2_core::workspace::handle::WorkspaceTokenResolve::Ambiguous { handles } => {
            WorkspaceResolve::Ambiguous { handles }
        }
        k2_core::workspace::handle::WorkspaceTokenResolve::Retired { handle } => {
            WorkspaceResolve::Retired { handle }
        }
        k2_core::workspace::handle::WorkspaceTokenResolve::Miss => WorkspaceResolve::Miss,
    }
}

/// First-arg target for `k2 msg` / `talk` / `read` (D12).
///
/// Federation `agent::host` is split in the CLI — not parsed here.
/// `sales-reviewer` is a workspace name (or fail), never a sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgTarget {
    WorkspaceCanonical {
        path: String,
    },
    Sidecar {
        path: String,
        /// The handle the caller sent (`3`, or a retired name).
        handle: String,
        conversation_key: String,
        /// The handle the chat answers at now (`reviewer`). Differs from
        /// `handle` after a rename; old addresses keep delivering.
        current_handle: String,
    },
    /// UUID that matched a live or durable session (provider or PTY id).
    Session {
        session_id: String,
    },
}

/// Why a `workspace/handle` token did not resolve, for the 404 hint
/// (the resolver itself returns `Option`). `None` for other shapes or
/// when the workspace half is unknown.
pub fn explain_unresolved_sidecar(first_arg: &str) -> Option<String> {
    let (ws, handle) =
        k2_core::workspace_session_handles::split_workspace_handle(first_arg.trim())?;
    if !k2_core::workspace_session_handles::is_address_token(ws) {
        return None;
    }
    let path = resolve_workspace(ws)?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let project_id = resolve_project_id(&conn, &path)?;
    k2_core::workspace_session_handles::resolve_for_delivery(&conn, &project_id, handle).err()
}

/// `ws/<current handle>` for a resolved sidecar, keeping the workspace
/// token the caller sent (workspace renames are not this PRD's).
pub fn current_sidecar_address(requested: &str, current_handle: &str) -> String {
    match k2_core::workspace_session_handles::split_workspace_handle(requested.trim()) {
        Some((ws, _)) => k2_core::workspace_session_handles::format_address(ws, Some(current_handle)),
        None => requested.trim().to_string(),
    }
}

/// Resolve `k2 msg` first arg. Order (D12):
/// 1. `agent::host` — not parsed here
/// 2. contains `/` and not an absolute path → `workspace/handle`
/// 3. UUID-shaped → live/durable session first; else project UUID
/// 4. else workspace → canonical
pub fn resolve_msg_target(first_arg: &str) -> Option<MsgTarget> {
    let token = first_arg.trim();
    if token.is_empty() {
        return None;
    }

    if let Some((ws, handle)) = k2_core::workspace_session_handles::split_workspace_handle(token) {
        // First segment must already be a handle token (D14). Pretty
        // names like `Sales Team/reviewer` are rejected (`/`).
        if !k2_core::workspace_session_handles::is_address_token(ws) {
            return None;
        }
        let path = resolve_workspace(ws)?;
        let project_id = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            resolve_project_id(&conn, &path)
        }?;
        // Delivery resolver: the old ordinal and every retired name keep
        // reaching the same chat (Thread survives a tab rename).
        let target = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace_session_handles::resolve_for_delivery(&conn, &project_id, handle)
                .ok()
        }?;
        return Some(MsgTarget::Sidecar {
            path,
            handle: handle.to_string(),
            conversation_key: target.conversation_key,
            current_handle: target.current_handle,
        });
    }

    if k2_core::workspace_session_handles::is_uuid_shape(token) {
        if session_exists_live_or_durable(token) {
            return Some(MsgTarget::Session {
                session_id: token.to_string(),
            });
        }
        if let Some(path) = resolve_workspace(token) {
            return Some(MsgTarget::WorkspaceCanonical { path });
        }
        return None;
    }

    resolve_workspace(token).map(|path| MsgTarget::WorkspaceCanonical { path })
}

fn session_exists_live_or_durable(session_id: &str) -> bool {
    if let Some(sid) = SessionId::parse(session_id) {
        if session_lookup::lookup_by_session_id(&sid).is_some() {
            return true;
        }
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, session_id)
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    if WorkspaceSession::get_by_session_id(&conn, session_id)
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    if k2_core::workspace_session_handles::project_id_for_session_id(&conn, session_id)
        .ok()
        .flatten()
        .is_some()
    {
        return true;
    }
    false
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
/// - UUID-shaped `from` (passport / `projects.id`) is rewritten to the
///   agent display name before framing — last-mile guard so `k2 msg` /
///   `k2 talk` never inject `[from <uuid>]` into a chat.
pub fn format_message(from: &str, text: &str, command: &str) -> String {
    let sender = stamp_name(from, "external");
    let text = &quote_stamp_lines(text);
    // thread-context-v1: K2's own line rides on its own line BEFORE the
    // stamp, never inside it (see [`command_with_lead_line`]).
    if let Some((cmd, lead)) = command.split_once(LEAD_LINE_SEP) {
        let (cmd, lead) = (cmd.trim(), lead.trim());
        let head = if cmd.is_empty() { lead.to_string() } else { format!("{cmd} {lead}") };
        return format!("{head}\n[from {sender}] {text}");
    }
    let command = command.trim();
    if command.is_empty() {
        format!("[from {sender}] {text}")
    } else {
        format!("{command} [from {sender}] {text}")
    }
}

/// Separates a composer slash-command from K2's own lead line inside the
/// `command` a delivery carries (thread-context-v1, the Board context
/// line). A control character: no composer command and no checked context
/// line can hold one, so only [`command_with_lead_line`] makes it.
pub const LEAD_LINE_SEP: char = '\u{1e}';

/// The `command` for a delivery whose PTY line starts with K2's own `lead`
/// line (thread-context-v1): [`format_message`] writes `<command> <lead>`
/// (the slash-command stays at the very front) and the `[from <name>]`
/// stamp line under it, so the lead is never inside the person's stamp.
pub fn command_with_lead_line(command: &str, lead: &str) -> String {
    format!("{}{LEAD_LINE_SEP}{}", command.trim(), lead.trim())
}

/// Composer slash-commands the GUI picker may send. `k2 msg --command`
/// still accepts arbitrary strings; this allowlist is composer-only.
pub const COMPOSER_SLASH_COMMANDS: &[&str] = &["/compact", "/goal"];

/// Normalize a composer `command` field.
///
/// - Empty / whitespace → `Ok(None)` (no slash prefix).
/// - Optional missing slash, case-insensitive exact match against
///   [`COMPOSER_SLASH_COMMANDS`] → `Ok(Some("/compact"))` or `"/goal"`.
/// - Anything else (`/exit`, `/loop`, `/compact now`, garbage) → `Err`.
///   Callers must NOT inject on `Err`.
pub fn normalize_composer_slash_command(raw: &str) -> Result<Option<&'static str>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let with_slash = if trimmed.starts_with('/') {
        trimmed.to_ascii_lowercase()
    } else {
        format!("/{}", trimmed.to_ascii_lowercase())
    };
    for &canonical in COMPOSER_SLASH_COMMANDS {
        if with_slash == canonical {
            return Ok(Some(canonical));
        }
    }
    Err(format!("unknown composer command: {trimmed}"))
}

/// Composer 1a (D3) — attributed bytes formatter for the session-scoped
/// `send-message` route. ALWAYS produces a non-empty `[from <name>]`
/// prefix. The sender is resolved server-side from the auth token by the
/// caller (NEVER from the request body); this helper just frames it.
///
/// Deliberately distinct from [`format_message`]:
/// - It does NOT fall back to `external` (that is `k2 msg`'s defense-in-
///   depth fallback and must stay there) — an empty `from` here falls
///   back to `owner` (1a is owner-token-only), so an injection is never
///   unattributed (red-team C3).
/// - Slash-commands use [`format_message_user_with_command`] so `/compact`
///   lands at the VERY FRONT (`/compact [from owner] hi`). Stuffing the
///   slash into `text` would yield `[from owner] /compact …` and the TUI
///   would not treat it as a command. Empty command is identical to this
///   helper's historical output.
pub fn format_message_user(from: &str, text: &str) -> String {
    format_message_user_with_command(from, text, "")
        .expect("empty composer slash command is always allowed")
}

/// Like [`format_message_user`], with an optional composer slash-command
/// prepended at the VERY FRONT (same shape as [`format_message`], owner
/// fallback). Unknown commands return `Err` and must not be injected.
pub fn format_message_user_with_command(
    from: &str,
    text: &str,
    command: &str,
) -> Result<String, String> {
    let sender = stamp_name(from, "owner");
    let text = &quote_stamp_lines(text);
    match normalize_composer_slash_command(command)? {
        None => Ok(format!("[from {sender}] {text}")),
        Some(cmd) => Ok(format!("{cmd} [from {sender}] {text}")),
    }
}

/// The words K2's own stamps start with (prd-postal-in-k2-v1 §5.4, Rosson's
/// 11-08 answer 7). A line of a message's body that starts like one could
/// pass for K2's label or another sender's.
pub const STAMP_STARTS: [&str; 6] = ["[from ", "[postal ", "[guest ", "[thread:", "[project:", "[inbox:"];

/// Does `line` start like one of K2's stamps? Leading whitespace and the
/// invisible and bidi characters a terminal doesn't show are skipped first,
/// and case is ignored.
pub fn looks_like_stamp(line: &str) -> bool {
    let seen = crate::overlay_routes::strip_untrusted_controls(line);
    let head: String = seen.trim_start().chars().take(10).collect::<String>().to_ascii_lowercase();
    STAMP_STARTS.iter().any(|s| head.starts_with(s))
}

/// A message body with every line after the first that starts like a stamp
/// ([`looks_like_stamp`]) quoted (`> `), on every delivery path (S2): no
/// body can start a line that reads as K2's label or another sender's. A
/// line starts after `\n`, `\r`, or a vertical tab, form feed, NEL, line or
/// paragraph separator; the breaks themselves are kept as sent.
pub fn quote_stamp_lines(text: &str) -> String {
    let is_break = |c: char| matches!(c, '\n' | '\r' | '\u{0b}' | '\u{0c}' | '\u{85}' | '\u{2028}' | '\u{2029}');
    if !text.contains(is_break) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len() + 8);
    let mut rest = text;
    let mut first = true;
    loop {
        let end = rest.find(is_break).unwrap_or(rest.len());
        let line = &rest[..end];
        if !first && looks_like_stamp(line) {
            out.push_str("> ");
        }
        out.push_str(line);
        first = false;
        if end == rest.len() {
            break;
        }
        let brk = rest[end..].chars().next().unwrap_or('\n');
        out.push(brk);
        rest = &rest[end + brk.len_utf8()..];
    }
    out
}

/// The name in `[from <name>]`: no line break or control character, so a
/// free-form `from` (`/v1`'s) can't start a line of its own.
fn stamp_name(from: &str, empty_fallback: &str) -> String {
    humanize_chat_from(from, empty_fallback).chars().filter(|c| !c.is_control()).collect()
}

/// Who a delivered line says it is from. A guest is its own kind of sender, not
/// a name: nothing in a `from` string (`/v1`'s is free-form) can make a
/// `[guest …]` stamp, and a guest's line never has the `[from X]` shape of a
/// person on this box (security review 2026-11-02).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender<'a> {
    Named(&'a str),
    /// [`guest_label`]'s words.
    Guest(&'a str),
    /// A Postal message from outside this server (prd-postal-in-k2-v1 S0,
    /// P17): the inside of K2's own label, `postal <address> · <badge>`
    /// ([`postal_label`]). Untrusted text: the label stands on its own line,
    /// every line of the body is quoted (the first too), the body loses
    /// every control, bidi and invisible character, and the inject
    /// chokepoint holds it when the terminal has bracketed paste off.
    Postal(&'a str),
}

impl<'a> Sender<'a> {
    fn name(self) -> &'a str {
        match self {
            Sender::Named(n) | Sender::Guest(n) | Sender::Postal(n) => n,
        }
    }

    /// Text from outside this server's people and agents, which the inject
    /// chokepoint treats as untrusted by type (never by sniffing the line).
    pub fn is_untrusted(self) -> bool {
        matches!(self, Sender::Postal(_))
    }
}

/// The inside of a Postal message's label: `postal <address> · <badge>`.
/// `address` is a parsed Postal address (`k2_core::postal::parse_address`)
/// and `badge` K2's own delivery id; neither can leave the brackets.
pub fn postal_label(address: &str, badge: &str) -> String {
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control() && !matches!(c, '[' | ']')).collect() };
    format!("postal {} · {}", clean(address).trim(), clean(badge).trim())
}

/// Every line of `text` quoted (`> `), the first included; an empty line is
/// a bare `>`. Every kind of line break counts as one (the Voice & Chat only
/// shape, security review M4), so no text can start a line of its own.
pub fn quote_every_line(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace(['\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}'], "\n");
    unified
        .split('\n')
        .map(|l| if l.is_empty() { ">".to_string() } else { format!("> {l}") })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The inside of a guest's stamp: `guest <who>`, with no way out of the brackets.
pub fn guest_label(who: &str) -> String {
    let clean: String = sanitize_inject_text(who).chars().filter(|c| !c.is_control() && !matches!(c, '[' | ']')).collect();
    format!("guest {}", clean.trim())
}

/// A guest's words with every line after the first quoted (`> `), so nothing
/// they type can start a line that reads as a person's or as K2's own. Every
/// kind of line break counts as one.
pub fn quote_guest_lines(text: &str) -> String {
    let unified = text.replace("\r\n", "\n").replace(['\r', '\u{0b}', '\u{0c}', '\u{85}', '\u{2028}', '\u{2029}'], "\n");
    let mut lines = unified.split('\n');
    let mut out = lines.next().unwrap_or("").to_string();
    for line in lines {
        out.push_str(if line.is_empty() { "\n>" } else { "\n> " });
        out.push_str(line);
    }
    out
}

/// [`format_message`] for any [`Sender`]. A guest has no composer command.
pub fn format_sender(sender: Sender<'_>, text: &str, command: &str) -> String {
    match sender {
        Sender::Named(from) => format_message(from, text, command),
        Sender::Guest(label) => format!("[{label}] {text}"),
        Sender::Postal(label) => {
            format!("[{label}]\n{}", quote_every_line(&crate::overlay_routes::strip_untrusted_controls(&sanitize_inject_text(text))))
        }
    }
}

/// The `[from …]` label a sender string renders as (handle for a UUID).
pub fn humanize_from(from: &str) -> String {
    humanize_chat_from(from, "external")
}

/// Last-mile chat attribution: stamp the **handle** (D18), never a
/// display name or a bare project/passport UUID.
fn humanize_chat_from(from: &str, empty_fallback: &str) -> String {
    let trimmed = from.trim();
    if trimmed.is_empty() {
        return empty_fallback.to_string();
    }
    if !is_uuid_shape(trimmed) {
        return trimmed.to_string();
    }
    // Passport / projects.id → handle.
    if let Some(path) = resolve_workspace(trimmed) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        if let Some(id) = resolve_project_id(&conn, &path) {
            if let Ok(handle) =
                k2_core::workspace_session_handles::workspace_address_name(&conn, &id)
            {
                if !handle.trim().is_empty() && !is_uuid_shape(handle.trim()) {
                    return handle;
                }
            }
        }
        if let Some(base) = Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.trim().is_empty() && !is_uuid_shape(s.trim()))
        {
            return base;
        }
    }
    // Unresolvable passport — still never emit the raw UUID into chat.
    empty_fallback.to_string()
}

fn is_uuid_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b[8] == b'-'
        && b[13] == b'-'
        && b[18] == b'-'
        && b[23] == b'-'
        && s.bytes()
            .enumerate()
            .all(|(i, c)| matches!(i, 8 | 13 | 18 | 23) || c.is_ascii_hexdigit())
}

// ─────────────────────────────────────────────────────────────────────
// Composer 1a — per-session injection lock + payload sanitization
// ─────────────────────────────────────────────────────────────────────

/// Composer 1a (D5 / red-team H2) — neutralize bracketed-paste
/// close-marker breakout. A payload containing a raw `ESC` (`0x1b`) —
/// most dangerously `ESC[201~` — would end paste mode early, so the
/// remainder of the "message" is interpreted as live keystrokes: exactly
/// the splice this feature exists to prevent. We strip EVERY raw `ESC`
/// byte BEFORE the bracketed-paste frame is added, so `[201~` (and any
/// other escape sequence) is left as inert literal text inside the
/// frame. Applied at the shared injection chokepoint so `k2 msg` payloads
/// get the same protection.
pub fn sanitize_inject_text(text: &str) -> String {
    text.chars().filter(|&c| c != '\u{1b}').collect()
}

/// A line a Voice & Chat only login sent (security review M4): its stamp
/// is `[guest <login> (Voice & Chat only)]` (`overlay_routes::post_voice_chat`).
pub fn is_voice_chat_line(payload: &str) -> bool {
    let first = payload.lines().next().unwrap_or("");
    first.starts_with("[guest ") && first.contains(&format!("{}]", crate::overlay_routes::VC_LABEL_SUFFIX))
}

/// Hold a Voice & Chat only line when the terminal doesn't have bracketed
/// paste on (M4): typed raw, each of its lines would submit by itself.
pub fn voice_chat_paste_hold(payload: &str, bracketed_paste: bool) -> bool {
    is_voice_chat_line(payload) && !bracketed_paste
}

// ─────────────────────────────────────────────────────────────────────
// "Your display name" — owner `from` resolution (D3)
// ─────────────────────────────────────────────────────────────────────

/// Max length of a resolved owner display name. Mirrors
/// [`k2_core::workspace::display::validate_display_name`]'s 64-char
/// ceiling so the attributed `[from <name>]` prefix can never balloon.
const OWNER_DISPLAY_NAME_MAX: usize = 64;

/// Sanitize a user-configured owner display name before it is embedded
/// into [`format_message_user`]'s `[from <name>]` prefix and injected
/// into a PTY.
///
/// The name flows into the SAME PTY-injection chokepoint as the
/// composer's text payload, so it gets the same anti-splice treatment
/// and then some: it must additionally stay on ONE line (a raw newline
/// would break the single-line prefix). Steps:
///   1. Strip raw `ESC` (`0x1b`) via the shared [`sanitize_inject_text`]
///      chokepoint (red-team H2 / D5 — no bracketed-paste breakout).
///   2. Drop every remaining ASCII/Unicode control char (newlines, CR,
///      tab, other C0/C1) — these would corrupt the one-line prefix.
///   3. Trim surrounding whitespace.
///   4. Cap to [`OWNER_DISPLAY_NAME_MAX`] chars.
///
/// Returns `None` when nothing usable survives, so the caller falls back
/// to the literal `"owner"`.
pub fn sanitize_owner_display_name(name: &str) -> Option<String> {
    // Step 1 — shared ESC-stripping chokepoint (anti-splice).
    let no_esc = sanitize_inject_text(name);
    // Step 2 — drop any remaining control chars (newlines/CR/tab/etc.).
    let cleaned: String = no_esc.chars().filter(|c| !c.is_control()).collect();
    // Step 3 — trim.
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Step 4 — cap length (char-wise, never splitting a multi-byte char).
    Some(trimmed.chars().take(OWNER_DISPLAY_NAME_MAX).collect())
}

/// Pure resolver (D3): given the configured `owner_display_name` exactly
/// as stored in app_settings, produce the attributed `from`. Sanitizes
/// for safe PTY injection; falls back to `"owner"` when unset / blank /
/// all-control. Kept separate from [`resolve_owner_from`] so it is unit-
/// testable without touching `~/.k2/settings.json`.
pub fn owner_from_or_default(configured: Option<&str>) -> String {
    configured
        .and_then(sanitize_owner_display_name)
        .unwrap_or_else(|| "owner".to_string())
}

/// D3 entrypoint for the `send-message` route: resolve the OWNER's
/// attributed `from`. 0.46.0 (Rosson 2026-10-17, identity = username
/// everywhere it matters): the shared stamp rule
/// `k2_core::person_name::sender_identity(Sender::OwnerToken)` — the
/// owner's linked account's USERNAME (the primary of `ownerAccounts`),
/// falling back to `"owner"` when nothing is linked. The old
/// `ownerDisplayName` setting is no longer read here; real names are
/// display only (`/cli/people/name`).
///
/// **D3 holds:** the name is read ONLY from server-side state — NEVER
/// from the request body — so a client cannot spoof the attribution.
pub fn resolve_owner_from() -> String {
    owner_from_or_default(Some(&k2_core::person_name::sender_identity_now(k2_core::person_name::Sender::OwnerToken)))
}

/// Composer 1a (D2) — max time a waiting injector blocks for the
/// per-session lock before giving up with a truthful `pty_stalled`.
/// Generously exceeds a single injection's timed sleeps (default ~520ms,
/// cap 5000ms sum(waitMs)) so legitimately-serialized sends (composer +
/// `k2 msg`) all land in order, while a truly stuck holder still
/// releases the waiter instead of pinning it forever.
const INJECT_LOCK_TIMEOUT: Duration = Duration::from_secs(8);

/// Outcome of a single locked injection attempt (Composer 1a).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InjectOutcome {
    /// Payload + submit CR written into a still-alive child.
    Delivered,
    /// The child died mid-injection → caller reports `pty_died`.
    PtyDied,
    /// The per-session injection lock did not free within the deadline
    /// (D2) → caller reports `pty_stalled`. The waiter never acquired
    /// the lock, so it holds nothing.
    Stalled,
    /// Slice 5 hard-safety rule (a): the recipient grok pane's screen
    /// shows an OPEN permission gate — NOTHING was written (the gate's
    /// default selection is always-approve; our submit CRs could grant
    /// it) → caller reports `hitl_gate_open`. Held, not eaten.
    GateHold,
}

/// Composer 1a (D1/M1) — per-RESOLVED-session injection locks.
///
/// Keyed by the live PTY [`SessionId`] (NOT project_id — one project can
/// own many terminals; the lock is per recipient PTY). The map mutex is
/// held only long enough to clone out the per-session `Arc`; the actual
/// serialization happens on that inner `parking_lot::Mutex` (which we
/// need for `try_lock_for`, D2). Entries are tiny (`Arc<Mutex<()>>`); we
/// never reap them.
static INJECT_LOCKS: LazyLock<PlMutex<HashMap<SessionId, Arc<PlMutex<()>>>>> =
    LazyLock::new(|| PlMutex::new(HashMap::new()));

/// Clone out the injection lock for `sid`, creating it on first use.
/// Every injector targeting the same resolved live session converges on
/// the SAME `Arc` — composer route, `k2 msg`'s live branch, and the
/// post-wake spawn delivery — so they are strictly one-at-a-time
/// ("one throat to choke", D1/D7).
fn lock_for(sid: &SessionId) -> Arc<PlMutex<()>> {
    let mut map = INJECT_LOCKS.lock();
    map.entry(*sid)
        .or_insert_with(|| Arc::new(PlMutex::new(())))
        .clone()
}

/// Issue #9 — per-PROJECT wake locks. Keyed by `project_id` (NOT session
/// id — a dormant workspace has no live session yet), these serialize the
/// wake/spawn path so two callers `talk`/`msg --wake`-ing the SAME dormant
/// peer don't each spawn a duplicate `claude --resume` session. The holder
/// spawns + waits-for-ready + injects; a concurrent waiter blocks on this
/// lock, then re-checks for the now-live session the first waker spawned
/// and injects into THAT instead of spawning again. Distinct from the
/// per-session [`INJECT_LOCKS`] (which serialize injection into an
/// already-live PTY); the two never nest in a cycle (wake-lock → then
/// inject-lock, never the reverse).
static WAKE_LOCKS: LazyLock<PlMutex<HashMap<String, Arc<PlMutex<()>>>>> =
    LazyLock::new(|| PlMutex::new(HashMap::new()));

/// Clone out the wake lock for `project_id`, creating it on first use.
fn wake_lock_for(project_id: &str) -> Arc<PlMutex<()>> {
    let mut map = WAKE_LOCKS.lock();
    map.entry(project_id.to_string())
        .or_insert_with(|| Arc::new(PlMutex::new(())))
        .clone()
}

/// Issue #9 — default ceiling for the post-wake readiness wait. A freshly
/// spawned `claude` TUI typically flips bracketed-paste mode on within a
/// few seconds; this is the generous upper bound after which we inject
/// best-effort anyway. Overridable per call via `--wake-timeout`.
pub const DEFAULT_WAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Floor for the liar / non-polling path only (`ready_via_bracketed_paste
/// == false`). Polling providers do not pre-sleep this before the
/// `?2004h` poll.
const WAKE_MIN_SETTLE: Duration = Duration::from_millis(400);

/// Post-mount headroom after a trustworthy `?2004h` 0→1; not used when
/// paste was already on, not used for liar profiles.
const POST_READY_SETTLE: Duration = Duration::from_millis(1000);

/// Issue #9 — poll cadence while waiting for the woken session to become
/// ready.
const WAKE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Post-wake quiescence: poll cadence for the "screen stopped changing"
/// settle that runs AFTER the bracketed-paste ready signal, before the
/// inject.
const WAKE_QUIESCE_POLL: Duration = Duration::from_millis(120);

/// Post-wake quiescence: how long the visible grid must stay unchanged
/// before we treat the woken TUI as settled (resumed-transcript replay
/// AND composer repaint both done). claude flips `?2004h` while it is
/// still finalizing a resumed conversation's render, so the ready signal
/// alone lands the paste in a pre-final frame that the repaint then wipes
/// — an idle composer's grid is stable, a mid-replay one is not, so a
/// short stability window distinguishes them.
const WAKE_QUIESCE_STABLE: Duration = Duration::from_millis(360);

// ─────────────────────────────────────────────────────────────────────
// Public entry — `deliver_live` (retry-wrapped)
// ─────────────────────────────────────────────────────────────────────
//
// Phase 2.1 wrap-up (0.39.0f): the pre-0.38.6 `deliver_to_inbox` helper
// (and its dependency on `k2_core::agents::commands::workspace_inbox_create`,
// which wrote to the retired `.k2so/work/inbox/` layout) was removed
// here. `msg` is strictly live-or-fail; new inbox-delivery callers
// should compose against `k2_core::inbox::compose` directly so they
// land in the canonical `.k2/inbox/` location the renderer reads.

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
///
/// **Issue #9 — wake gating.** `wake` controls whether a *dormant
/// canonical session* (workspace HAS a resolvable primary agent but no
/// live PTY) is auto-woken before delivery:
/// - `wake == true` (the `k2 talk` default, and `k2 msg --wake`): resume
///   the saved session (or fresh-spawn), wait until it is READY (bounded
///   by `wake_timeout`), then inject. The response carries `woke: true` +
///   `wake_ms` so the caller can attribute the latency.
/// - `wake == false` (the `k2 msg` default): a dormant-but-wakeable peer
///   returns `dormant_no_wake` (distinct from `no_agent_mode`) instead of
///   spawning — keep `msg` a dumb blind send unless the caller opts in.
///
/// A workspace with NO resolvable primary agent always returns
/// `no_agent_mode` (it cannot be woken regardless of `wake`).
pub fn deliver_live(
    workspace_token: &str,
    text: &str,
    from: &str,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
) -> MsgResponse {
    deliver_live_with_via(
        workspace_token,
        text,
        from,
        command,
        wake,
        wake_timeout,
        "msg",
    )
}

/// Same as [`deliver_live`] but stamps chatter `via` (`msg` | `talk` | `inbox` | `v1`).
/// `compose`, `thread`, `chat` (the Sew delivery hub) and `mail` (W5: a mail
/// line must not put a subject in Chatter) record no Chatter line.
pub fn deliver_live_with_via(
    workspace_token: &str,
    text: &str,
    from: &str,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
    via: &str,
) -> MsgResponse {
    deliver_live_inner(workspace_token, text, Sender::Named(from), command, wake, wake_timeout, via, None)
}

/// [`deliver_live_with_via`] for any [`Sender`].
pub(crate) fn deliver_live_as(
    workspace_token: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
    via: &str,
) -> MsgResponse {
    deliver_live_inner(workspace_token, text, from, command, wake, wake_timeout, via, None)
}

/// A send's `msg_deliveries` receipt plus what the Chatter line keeps:
/// the chatter `from`, the `m_…` id and the sender's one-line summary.
#[derive(Clone, Copy)]
struct Receipt<'a> {
    from: &'a str,
    delivery_id: &'a str,
    summary: Option<&'a str>,
}

/// A live send that wrote a `msg_deliveries` receipt
/// ([`crate::hire_routes::deliver_recorded`]): `stamp` is what the
/// recipient sees (`[from label · m_…]`, or a Postal label); the Chatter
/// line keeps the caller's own `from` and the receipt's `delivery_id`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn deliver_live_with_receipt(
    workspace_token: &str,
    text: &str,
    stamp: Sender<'_>,
    from: &str,
    delivery_id: &str,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
    via: &str,
    summary: Option<&str>,
) -> MsgResponse {
    deliver_live_inner(
        workspace_token,
        text,
        stamp,
        command,
        wake,
        wake_timeout,
        via,
        Some(Receipt { from, delivery_id, summary }),
    )
}

/// `receipt`: `(chatter from, delivery id)` when the send has a
/// `msg_deliveries` row.
#[allow(clippy::too_many_arguments)]
fn deliver_live_inner(
    workspace_token: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
    via: &str,
    receipt: Option<Receipt<'_>>,
) -> MsgResponse {
    // Resolve once (D12). Unknown token is permanent; surface immediately.
    let target = match resolve_msg_target(workspace_token) {
        Some(t) => t,
        None => {
            let suggest_token =
                k2_core::workspace_session_handles::split_workspace_handle(workspace_token)
                    .map(|(ws, _)| ws)
                    .unwrap_or(workspace_token);
            match resolve_workspace_detailed(suggest_token) {
                WorkspaceResolve::Ambiguous { handles } => {
                    let mut resp = MsgResponse::fail(MsgReason::WorkspaceNotFound);
                    resp.hint = Some(format!(
                        "AMBIG: '{suggest_token}' matches multiple workspaces. Use a unique handle: {}",
                        handles.join(", ")
                    ));
                    return resp;
                }
                WorkspaceResolve::Retired { handle } => {
                    let mut resp = MsgResponse::fail(MsgReason::WorkspaceNotFound);
                    resp.hint = Some(format!(
                        "{} Run `k2 connections list` to see current workspaces.",
                        k2_core::workspace::handle::retired_handle_message(&handle)
                    ));
                    return resp;
                }
                _ => {}
            }
            let mut resp = MsgResponse::fail(MsgReason::WorkspaceNotFound);
            let suggestion = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::connections::suggest_project_name(&conn, suggest_token)
            };
            if workspace_token.contains("::") {
                // 0.45.6: a federated `agent::host` address never resolves
                // in this server's workspace list — say so instead of
                // pointing at the local connections list.
                resp.hint = Some(format!(
                    "'{workspace_token}' is an address on another server; this call only reaches workspaces on this server. Use `k2 msg {workspace_token} \"text\"` for a live message, or `k2 msg {workspace_token} --inbox-wake ...` to deliver a package and wake the agent there."
                ));
            } else if workspace_token.contains('/') && !k2_core::paths::is_path_token(workspace_token) {
                resp.hint = Some(match explain_unresolved_sidecar(workspace_token) {
                    Some(why) => format!("{why}. Use `k2 msg <workspace>/<handle>` for a sidecar (`k2 sessions` lists the addresses)."),
                    None => format!(
                        "Unknown workspace/handle '{workspace_token}'. Use `k2 msg <workspace>` for the primary or `k2 msg <workspace>/<handle>` for a sidecar (`k2 sessions` lists the addresses). Hyphenated names like `sales-reviewer` are workspaces, not sidecars."
                    ),
                });
            } else if let Some(s) = suggestion {
                resp.hint = Some(format!(
                    "Unknown workspace '{workspace_token}' — did you mean '{s}'? Run `k2 connections list` to see available workspaces."
                ));
            }
            return resp;
        }
    };

    let mut last: Option<MsgResponse> = None;
    for attempt in 1..=MAX_ATTEMPTS {
        let mut result = match &target {
            MsgTarget::WorkspaceCanonical { path } => {
                attempt_delivery(path, text, from, command, wake, wake_timeout)
            }
            MsgTarget::Sidecar {
                path,
                handle,
                conversation_key,
                ..
            } => attempt_sidecar_delivery(
                path,
                handle,
                conversation_key,
                text,
                from,
                command,
                wake,
                wake_timeout,
            ),
            MsgTarget::Session { session_id } => {
                attempt_session_delivery(session_id, text, from, command, wake, wake_timeout)
            }
        };
        result.attempts = attempt;
        if let MsgTarget::Sidecar { handle, current_handle, .. } = &target {
            if handle != current_handle {
                result.addr = Some(current_sidecar_address(workspace_token, current_handle));
                result.moved_from = Some(workspace_token.trim().to_string());
            }
        }

        if result.success {
            // Human Thread compose / card inject already stored on the
            // Thread collection — do not also stamp Chatter (that's k2 msg).
            // S31 `via = chat` (the Sew delivery hub): chat text never goes
            // on disk, so no Chatter line either.
            if via != "compose" && via != "thread" && via != "chat" && via != "mail" {
                let (chatter_from, delivery_id, summary) = match receipt {
                    Some(r) => (r.from, Some(r.delivery_id), r.summary),
                    None => (from.name(), None, None),
                };
                crate::overlay_routes::record_inject_chatter(
                    workspace_token,
                    chatter_from,
                    text,
                    via,
                    delivery_id,
                    summary,
                );
            }
            return result;
        }

        // Permanent reasons short-circuit — waiting won't fix them.
        // `dormant_no_wake` is permanent for THIS call: re-firing without
        // a wake request will keep returning it (#9).
        if let Some(reason) = result.reason.as_deref() {
            let permanent = matches!(
                reason,
                "workspace_not_found"
                    | "no_agent_mode"
                    | "dormant_no_wake"
                    | "folder_trust_pending"
                    | "folder_trust_denied"
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

/// Issue #9 — the three distinct no-live-session states the cascade must
/// disambiguate once it has exhausted the live-inject branches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DormantDecision {
    /// No agent configured for the workspace → un-wakeable; `no_agent_mode`
    /// with a clear remedy. Never spawns.
    NoAgent,
    /// An agent EXISTS but the caller did NOT request a wake →
    /// `dormant_no_wake`. The peer is wakeable; point them at `--wake`.
    NoWake,
    /// An agent EXISTS AND a wake was requested → resume/fresh-spawn
    /// and deliver.
    Wake,
}

/// Pure classifier for the dormant branch (#9).
///
/// **#9 re-fix:** `agent_exists` is now a DB fact decided by the caller —
/// a saved canonical `workspace_sessions.session_id` OR
/// `projects.agent_enabled == 1` — NOT whether the persona FILE
/// `.k2/agent/AGENT.md` is present. The persona file is frequently MISSING
/// for configured, dormant workspaces post-AGENTS.md cutover (only the
/// generated root-mirror `<ws>/AGENT.md` survives), so the prior
/// `find_primary_agent`-based gate wrongly classified textbook wake cases
/// (saved session + `agent_enabled=1`) as `no_agent_mode`.
///
/// Kept separate from [`attempt_delivery`] so the three-state branching is
/// unit-testable without touching the DB, the filesystem, or the (real)
/// spawn path.
fn classify_dormant(agent_exists: bool, wake: bool) -> DormantDecision {
    match (agent_exists, wake) {
        (false, _) => DormantDecision::NoAgent,
        (true, false) => DormantDecision::NoWake,
        (true, true) => DormantDecision::Wake,
    }
}

/// Issue #9 re-fix — resolve the name used as the daemon session-map key +
/// heartbeat lock label when waking a dormant workspace.
///
/// **Why a fallback exists:** existence is now decided from the DB (a saved
/// canonical session OR `agent_enabled=1`), so we reach the spawn path even
/// when the persona FILE `<ws>/.k2/agent/AGENT.md` is missing — the exact
/// condition that made the prior fix mis-classify configured agents as
/// `no_agent_mode`. The name must NOT bail us back there.
///
/// **Resolution order:**
///   1. The persona's declared `name:` via [`find_primary_agent`], when the
///      AGENT.md file is present (preserves the original spawn name).
///   2. Fallback = the workspace folder basename — this matches the product
///      default ("agent display name defaults to the workspace basename")
///      so the woken session is labeled consistently with the rest of the
///      app (e.g. `testing-canonical`).
///   3. Last-resort fallback = the `agent_mode` role label
///      (`custom`/`manager`/`k2so`), else the literal `agent`, for the
///      degenerate case of an empty/`/` workspace path.
///
/// The name does NOT determine which conversation resumes — that is keyed
/// solely by `claude --resume <session_id>` — so any stable, recognizable
/// label is correct here.
fn resolve_spawn_name(project_path: &str, agent_mode: &str) -> String {
    // 1. Persona file present → use its declared name.
    if let Some(name) = k2_core::workspace::agent_identity::find_primary_agent(project_path) {
        if !name.trim().is_empty() {
            return name;
        }
    }
    // 2. Workspace folder basename (the product-default display name).
    if let Some(base) = Path::new(project_path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.trim().is_empty())
    {
        return base;
    }
    // 3. Degenerate path → role label, else a generic constant.
    match agent_mode {
        // Stage A dual-read: `k2` is a synonym of legacy stored `k2so`.
        "custom" | "manager" | "k2so" | "k2" => agent_mode.to_string(),
        _ => "agent".to_string(),
    }
}

/// One delivery attempt. Returns `MsgResponse` with `attempts: 1`;
/// the [`deliver_live`] retry wrapper rewrites it on retry.
fn attempt_delivery(
    project_path: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
) -> MsgResponse {
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
                return inject_live(
                    &live,
                    text,
                    from,
                    command,
                    "active_terminal_id",
                    &project_id,
                );
            }
        }
        // Stale stamp — clear so downstream branches don't re-trip on it.
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::clear_active_terminal_id(&conn, &project_id);
    }

    // Branch 1b: argv-scan fallback for the cold-start case (any live
    // PTY whose argv claims the saved session id). Slice 3b: the match
    // is grammar-aware (`provider_resume::argv_references_session`) so
    // subcommand-style resumes (`codex resume <id>`) are found too.
    if let Some(saved_sid) = saved_session.as_deref() {
        for (_n, live) in session_lookup::snapshot_all() {
            if k2_core::workspace::provider_resume::argv_references_session(&live.args(), saved_sid)
            {
                return inject_live(&live, text, from, command, "argv_scan", &project_id);
            }
        }
    }

    // ── Issue #9: classify the no-live-session state BEFORE the spawn
    // branches, so the three states the ticket calls out are distinct:
    //   • no agent configured            → no_agent_mode (un-wakeable;
    //     clear remedy `k2 settings --mode custom` / register). Do NOT spawn.
    //   • agent configured + wake NOT requested → dormant_no_wake (the peer
    //     CAN be woken; the caller opted out — point them at `--wake`).
    //   • agent configured + wake requested → wake it (resume or fresh) and
    //     deliver, waiting until READY first.
    //
    // #9 RE-FIX — existence is a DB fact, NOT the persona file. The prior
    // fix gated existence on `find_primary_agent`, which reads the persona
    // FILE `<ws>/.k2/agent/AGENT.md`. That file is frequently MISSING for
    // configured, dormant workspaces post-AGENTS.md cutover (only the
    // generated root-mirror `<ws>/AGENT.md` survives), so a textbook wake
    // case — a saved canonical session with `agent_enabled=1` and no live
    // PTY — wrongly returned `no_agent_mode`. The authoritative signals are
    // (a) a saved canonical `workspace_sessions.session_id` and (b)
    // `projects.agent_enabled=1`; resuming by `--resume <session_id>`
    // restores the conversation regardless of the persona file.
    let project_row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        Project::get(&conn, &project_id).ok()
    };
    let agent_enabled = project_row
        .as_ref()
        .map(|p| p.agent_enabled == 1)
        .unwrap_or(false);
    let agent_mode = project_row
        .as_ref()
        .map(|p| p.agent_mode.clone())
        .unwrap_or_else(|| "off".to_string());
    let agent_exists = saved_session.is_some() || agent_enabled;

    match classify_dormant(agent_exists, wake) {
        DormantDecision::NoAgent => return MsgResponse::fail(MsgReason::NoAgentMode),
        DormantDecision::NoWake => return MsgResponse::fail(MsgReason::DormantNoWake),
        DormantDecision::Wake => {}
    }

    // Wake confirmed. Resolve the spawn session-map name with a fallback so
    // a missing persona file never bounces us back to `no_agent_mode`
    // (the bug being fixed): the name is only the daemon session-map key +
    // heartbeat lock label — `--resume <session_id>` restores the actual
    // conversation regardless of the name.
    let agent_name = resolve_spawn_name(project_path, &agent_mode);

    // Branches 2+3: wake. The canonical resume resolver (Slice 3b)
    // decides resume-vs-fresh AND which agent binary speaks — the
    // stored harness wins for an existing canonical session, the
    // workspace/global default governs a fresh one.
    wake_and_fire(
        project_path,
        &project_id,
        &agent_name,
        text,
        from,
        command,
        wake_timeout,
    )
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

// ── Slice 5 — grok permission-gate hold (hard-safety rule a) ─────────

/// How many bottom screen rows the gate scan reads. The gate's option
/// rows + footer render near the bottom of grok's TUI; bounding the
/// scan keeps old transcript content from false-holding.
const GROK_GATE_SCAN_ROWS: usize = 15;

/// Is `live` a grok pane sitting on an OPEN permission gate?
///
/// Two-factor: (1) the pane's spawn command must resolve to the grok
/// provider — so a claude/other pane whose transcript QUOTES the gate
/// strings can never be held — and (2) the bottom of the rendered
/// screen carries a gate marker ([`screen_shows_grok_gate`]). The
/// grok study's title signal (`⚠ Action Required - ` prefix) is not
/// consulted here because the daemon's label state machine may hold a
/// user-locked label; the screen markers are always present while the
/// gate is open.
fn grok_gate_open(live: &session_lookup::LiveSession) -> bool {
    let is_grok = live
        .command()
        .and_then(|c| k2_core::workspace::provider_resume::provider_resume_for_command(&c))
        .map(|p| p.provider == "grok")
        .unwrap_or(false);
    if !is_grok {
        return false;
    }
    screen_shows_grok_gate(&live.visible_text_rows())
}

/// Pure screen classifier for the grok permission gate (verbatim
/// markers from the 2026-07 study, matched case-insensitively in the
/// last [`GROK_GATE_SCAN_ROWS`] rows):
///   - the gate footer `1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel`
///     (`ctrl+o:yolo` is grok-unique), or
///   - the always-approve radio row
///     `1 (●) Yes, and don't ask again` (any radio state).
fn screen_shows_grok_gate(rows: &[String]) -> bool {
    rows.iter().rev().take(GROK_GATE_SCAN_ROWS).any(|row| {
        let lower = row.to_lowercase();
        lower.contains("ctrl+o:yolo")
            || lower.contains("(●) yes, and don't ask again")
            || lower.contains("(○) yes, and don't ask again")
    })
}

/// Inject `payload` + submit into `live`, with paste framing and an
/// insurance Enter for latency — **serialized by the per-session
/// injection lock** (Composer 1a, D1/D2/D5/D7).
///
/// This is the single chokepoint EVERY injector funnels through —
/// `inject_live` (`k2 msg` branch 1/1b), `spawn_and_inject`'s post-wake
/// delivery (`k2 msg` branches 2/3), and the composer `send-message`
/// route — so making it own the lock makes them all mutually exclusive
/// against one PTY ("one throat to choke"). The lock is keyed on the
/// RESOLVED `live.session_id()` (M1), so a re-spawned/woken session and
/// the human composer serialize on the same key.
///
/// D2 is satisfied in BOTH halves:
/// - **Waiter bound:** `try_lock_for(INJECT_LOCK_TIMEOUT)` — a waiter
///   gives up and returns [`InjectOutcome::Stalled`] (→ `pty_stalled`)
///   instead of blocking forever; it never acquires the lock so it holds
///   nothing.
/// - **Holder safety (non-blocking write):** satisfied structurally —
///   `LiveSession::write` enqueues onto alacritty's event-loop channel
///   and returns immediately (`daemon_pty.rs` `write`, documented
///   "Non-blocking"); it does NOT write the PTY master FD directly. So a
///   `SIGSTOP`ped/backpressured child fills alacritty's IO-thread pipe,
///   never the lock holder's `write()` call — the holder only ever sleeps
///   the bounded ~520ms of timed settles, then releases. (There is no raw
///   master-FD `O_NONBLOCK` flag to flip from here; the architecture
///   removes the wedge this guards against.)
fn inject_and_submit(live: &session_lookup::LiveSession, payload: &str) -> InjectOutcome {
    inject_and_submit_with_timeout(live, payload, INJECT_LOCK_TIMEOUT)
}

/// [`inject_and_submit`] for a payload whose sender's type says whether it
/// is untrusted ([`Sender::is_untrusted`]): the chokepoint strips an
/// untrusted one and holds it without bracketed paste.
fn inject_and_submit_as(live: &session_lookup::LiveSession, payload: &str, untrusted: bool) -> InjectOutcome {
    inject_and_submit_with_timeout_as(live, payload, INJECT_LOCK_TIMEOUT, untrusted)
}

/// 0.45.6 — heartbeat wake delivery through the SAME guarded path `k2 msg`
/// uses, replacing the heartbeat's old raw `write(body)` + fixed sleep.
///
/// - `after_spawn = Some((profile, ceiling))` — a just-spawned/resumed
///   session: wait for the provider's readiness signal and for the screen
///   to settle ([`deliver_post_wake`]) before pasting. Writing during the
///   TUI's startup render is how a wake body lost its head: the composer
///   repaints and drops what was already typed, and only the tail that
///   arrives after the repaint survives.
/// - `None` — an already-live session: the locked, framed inject only.
///
/// `Ok(())` means the payload + submit were written into a live child
/// ([`InjectOutcome::Delivered`]); `Err` carries the canonical reason
/// code (`pty_died` / `pty_stalled` / `hitl_gate_open`) so the heartbeat
/// can record a failure instead of `fired` (GH #61).
pub(crate) fn deliver_heartbeat_wake(
    live: &session_lookup::LiveSession,
    payload: &str,
    after_spawn: Option<(&k2_core::workspace::provider_resume::InjectionProfile, Duration)>,
) -> Result<(), &'static str> {
    let outcome = match after_spawn {
        Some((profile, ceiling)) => deliver_post_wake(live, payload, ceiling, profile),
        None => inject_and_submit(live, payload),
    };
    match outcome {
        InjectOutcome::Delivered => Ok(()),
        InjectOutcome::PtyDied => Err(MsgReason::PtyDied.code()),
        InjectOutcome::Stalled => Err(MsgReason::PtyStalled.code()),
        InjectOutcome::GateHold => Err(MsgReason::HitlGateOpen.code()),
    }
}

/// A daemon notice (`via=notice`, TR8) into an already-live session: the
/// same serialized inject + submit as a `k2 msg` live delivery. Never
/// wakes, records no Chatter, starts no Thread turn, is not stored.
pub(crate) fn inject_notice(live: &session_lookup::LiveSession, line: &str) -> bool {
    matches!(inject_and_submit(live, line), InjectOutcome::Delivered)
}

/// This workspace's newest tab row for a sidecar conversation (its pane
/// id, for the `session_address_changed` event).
pub(crate) fn pane_group_for_sidecar(project_id: &str, conversation_key: &str) -> Option<String> {
    project_tab_rows_for_conversation(project_id, conversation_key)
        .into_iter()
        .next()
        .map(|row| row.pane_group_id)
}

/// [`inject_and_submit`] with an explicit lock-acquire deadline. The
/// production wrapper passes [`INJECT_LOCK_TIMEOUT`]; tests pass a short
/// deadline to exercise the `Stalled` path deterministically without a
/// real wedged child.
fn inject_and_submit_with_timeout(
    live: &session_lookup::LiveSession,
    payload: &str,
    lock_timeout: Duration,
) -> InjectOutcome {
    inject_and_submit_with_timeout_as(live, payload, lock_timeout, false)
}

fn inject_and_submit_with_timeout_as(
    live: &session_lookup::LiveSession,
    payload: &str,
    lock_timeout: Duration,
    untrusted: bool,
) -> InjectOutcome {
    // D1/D2 — acquire the per-session lock on the RESOLVED live session,
    // bounded by the deadline so a stuck holder can't pin this waiter.
    let lock = lock_for(&live.session_id());
    let _guard = match lock.try_lock_for(lock_timeout) {
        Some(g) => g,
        None => {
            log_debug!(
                "[msg/inject] session={} injection lock held past deadline — pty_stalled",
                live.session_id()
            );
            return InjectOutcome::Stalled;
        }
    };
    inject_framed_locked(live, payload, untrusted)
}

/// The actual framed write + submit. ALWAYS called while holding the
/// per-session injection lock (via [`inject_and_submit_with_timeout`]).
///
/// Slice 5 safety note (rule c): the D5 sanitizer below strips EVERY
/// raw `ESC` from the payload before the `ESC[200~ … ESC[201~` frame is
/// added, so no payload byte can close the paste early — every `j`,
/// digit, or other hotkey-shaped character in the message stays INSIDE
/// bracketed paste and reaches the TUI as literal text, never as a
/// keystroke (the gemini study showed a bare injected `j` moving a
/// trust radio; the paste framing is what prevents that here). When the
/// recipient does NOT advertise bracketed paste the payload goes raw —
/// which is today's behavior for plain composers; the grok gate-hold
/// above is the guard for the one screen where raw keystrokes are
/// catastrophic. Pinned by
/// `framed_injection_payload_cannot_escape_the_paste_frame`.
fn inject_framed_locked(live: &session_lookup::LiveSession, payload: &str, untrusted: bool) -> InjectOutcome {
    // Slice 5 hard-safety rule (a): NEVER write into a grok pane whose
    // screen shows an open permission gate. Grok's gate default is
    // "1 (●) Yes, and don't ask again for anything (always-approve
    // mode)" — the submit CR (or the insurance CR) landing on it would
    // grant blanket approval. Hold instead; the caller reports
    // `hitl_gate_open` and the sender re-fires after a human resolves
    // the gate. Checked INSIDE the lock so a gate that opens while a
    // send is queued is still caught.
    if grok_gate_open(live) {
        log_debug!(
            "[msg/inject] session={} grok permission gate open — holding delivery (hitl_gate_open)",
            live.session_id()
        );
        return InjectOutcome::GateHold;
    }
    // Security review M4: a Voice & Chat only post (an untrusted person) is
    // typed in only inside bracketed paste, so none of its lines can submit
    // on its own; without it the line is held, never typed raw.
    // P17: a Postal message is untrusted by its sender's type; a Voice &
    // Chat only post by its stamp.
    let untrusted = untrusted || is_voice_chat_line(payload);
    if untrusted && !live.bracketed_paste_active() {
        log_debug!(
            "[msg/inject] session={} untrusted line (Voice & Chat only or Postal) held: the terminal has bracketed paste off",
            live.session_id()
        );
        return InjectOutcome::GateHold;
    }
    // D5 — strip raw ESC BEFORE framing so an embedded `ESC[201~` can't
    // close the paste early and splice live keystrokes. Sanitize at the
    // paste payload, not at Return; grok_gate_open ran once before step 1.
    // M4: a Voice & Chat only line also loses every other C0/C1 control
    // (but `\n`) and the bidi overrides, here at the one chokepoint.
    let clean = if untrusted {
        crate::overlay_routes::strip_untrusted_controls(&sanitize_inject_text(payload))
    } else {
        sanitize_inject_text(payload)
    };
    let body: Vec<u8> = if live.bracketed_paste_active() {
        let mut b = Vec::with_capacity(clean.len() + 12);
        b.extend_from_slice(b"\x1b[200~");
        b.extend_from_slice(clean.as_bytes());
        b.extend_from_slice(b"\x1b[201~");
        b
    } else {
        clean.into_bytes()
    };

    let flow = match k2_core::db_ops::resolve_inject_flow_for_program(live.command().as_deref()) {
        Ok(steps) => steps,
        Err(e) => {
            log_debug!(
                "[msg/inject] session={} inject_flow resolve failed ({e}) — using default",
                live.session_id()
            );
            k2_core::inject_flow::default_inject_flow()
        }
    };
    let actions = k2_core::inject_flow::compile_inject_flow(&flow, &body);
    for action in actions {
        match action {
            k2_core::inject_flow::InjectAction::Write(bytes) => {
                if live.write(&bytes).is_err() {
                    return InjectOutcome::PtyDied;
                }
            }
            k2_core::inject_flow::InjectAction::Sleep(ms) => {
                if ms > 0 {
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }
            k2_core::inject_flow::InjectAction::CheckAlive => {
                if !live.is_child_alive() {
                    return InjectOutcome::PtyDied;
                }
            }
        }
    }
    InjectOutcome::Delivered
}

// ─────────────────────────────────────────────────────────────────────
// Composer 1a — session-scoped `POST /cli/terminal/send-message`
// ─────────────────────────────────────────────────────────────────────

/// Deliver `text` to a specific LIVE PTY session, attributed to `from`.
///
/// The Phase 1a `send-message` route entrypoint. Contrast with
/// [`deliver_live`] (`k2 msg`), which is workspace-scoped and carries a
/// wake/spawn cascade:
///
/// - **Live-session-only (M1).** This NEVER spawns. A `session_id` that
///   is malformed or no longer maps to a live session returns `pty_died`;
///   the wake behavior is owned solely by `k2 msg` (D7).
/// - **Identity (D3).** `from` is resolved by the caller from the auth
///   token (NEVER the request body) and is already non-empty; it is
///   framed by [`format_message_user`] — the attributed branch, NOT
///   `k2 msg`'s `external` fallback.
/// - **Anti-splice (D1/D2/D5).** Delivery funnels through the shared,
///   bounded, ESC-sanitized [`inject_and_submit`], so the composer and
///   `k2 msg` are strictly one-at-a-time on the same PTY.
///
/// Tickets and other no-command callers keep this 3-arg form (empty
/// command). The composer picker uses
/// [`send_message_to_session_with_command`]. `k2 workspace swap-canonical`
/// sends its handoff through this form.
pub fn send_message_to_session(session_id: &str, from: &str, text: &str) -> MsgResponse {
    send_message_to_session_with_command(session_id, from, text, "")
        .expect("empty composer slash command is always allowed")
}

/// [`send_message_to_session`] for a session that has only just started:
/// waits for `provider`'s readiness signal and a settled screen first
/// ([`deliver_post_wake`]). A paste during a TUI's start-up can be dropped
/// while the write itself succeeds, and a refreshed chat's note must arrive.
/// Blocks up to [`DEFAULT_WAKE_TIMEOUT`]; `messageDelivered` still means
/// "written to a live child", not "read".
pub fn send_message_after_spawn(session_id: &str, from: &str, text: &str, provider: &str) -> MsgResponse {
    let payload = format_message_user_with_command(from, text, "").expect("empty composer slash command is always allowed");
    let Some(live) = SessionId::parse(session_id).and_then(|sid| session_lookup::lookup_by_session_id(&sid)) else {
        return MsgResponse::fail(MsgReason::PtyDied);
    };
    let profile = k2_core::workspace::provider_resume::injection_profile_for_provider(provider);
    let ceiling = if cfg!(test) { Duration::from_millis(300) } else { DEFAULT_WAKE_TIMEOUT };
    match deliver_post_wake(&live, &payload, ceiling, &profile) {
        InjectOutcome::Delivered => MsgResponse::ok(live.session_id().to_string(), "send_message"),
        InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
        InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
        InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
    }
}

/// Composer `send-message` with an allowlisted slash-command.
///
/// Unknown commands return `Err` and do **not** inject. Empty/whitespace
/// command is identical to [`send_message_to_session`].
pub fn send_message_to_session_with_command(
    session_id: &str,
    from: &str,
    text: &str,
    command: &str,
) -> Result<MsgResponse, String> {
    let payload = format_message_user_with_command(from, text, command)?;
    // M1 — resolve to a LIVE session; never spawn.
    let sid = match SessionId::parse(session_id) {
        Some(s) => s,
        None => return Ok(MsgResponse::fail(MsgReason::PtyDied)),
    };
    let live = match session_lookup::lookup_by_session_id(&sid) {
        Some(l) => l,
        None => return Ok(MsgResponse::fail(MsgReason::PtyDied)),
    };

    Ok(match inject_and_submit(&live, &payload) {
        InjectOutcome::Delivered => MsgResponse::ok(live.session_id().to_string(), "send_message"),
        InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
        InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
        InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
    })
}

/// Type one of the CLI's own commands (`/model opus`) into a LIVE session
/// and submit it, unframed (a `[from …]` stamp would make it a message, not
/// a command). The same locked, ESC-sanitized [`inject_and_submit`] the
/// composer's `/compact` and `/goal` ride. Never spawns. Used by
/// `/cli/sessions/model` (0.46.13).
/// `Err` is the refusal code (`pty_died`, `pty_stalled`, `hitl_gate_open`).
pub fn inject_cli_command(session_id: &SessionId, command: &str) -> Result<(), &'static str> {
    let live = session_lookup::lookup_by_session_id(session_id).ok_or(MsgReason::PtyDied.code())?;
    match inject_and_submit(&live, command) {
        InjectOutcome::Delivered => Ok(()),
        InjectOutcome::PtyDied => Err(MsgReason::PtyDied.code()),
        InjectOutcome::Stalled => Err(MsgReason::PtyStalled.code()),
        InjectOutcome::GateHold => Err(MsgReason::HitlGateOpen.code()),
    }
}

/// Persist compose-bar send history. Called after a successful
/// `POST /cli/terminal/send-message` (and overlay `thread/post` via=compose
/// records the same table). Tickets go through
/// [`send_message_to_session`] and must never call this.
pub fn persist_compose_send_if_delivered(success: bool, cwd: &str, from: &str, text: &str) {
    if !success {
        return;
    }
    let _ = k2_core::workspace_compose_history::record_compose_send_for_cwd(cwd, text, from);
}

/// After a successful inject: resolve the live session cwd and record.
/// Unknown/missing session or unregistered path: skip (send already
/// succeeded).
pub fn persist_compose_send_after_success(session_id: &str, from: &str, text: &str) {
    let Some(sid) = SessionId::parse(session_id) else {
        return;
    };
    let Some(live) = session_lookup::lookup_by_session_id(&sid) else {
        return;
    };
    persist_compose_send_if_delivered(true, &live.cwd(), from, text);
}

/// `GET /cli/terminal/compose-history` body. Newest first. Unknown
/// workspace → empty `items` (not an error).
pub fn compose_history_response(project_id: &str, workspace_path: &str) -> String {
    let items = k2_core::workspace_compose_history::list_for_query(project_id, workspace_path)
        .unwrap_or_default();
    serde_json::to_string(&serde_json::json!({ "items": items }))
        .unwrap_or_else(|_| "{\"items\":[]}".to_string())
}

/// Host sessions F1 (prd-v1-api-completion §3) — deliver a RAW payload into
/// a LIVE session, optionally waiting for TUI readiness first.
///
/// The `/v1/w/<ws>/host-sessions` family's injector: unlike
/// [`send_message_to_session`] the payload is UNFRAMED-by-attribution (no
/// `[from]` prefix — the API caller IS the session's principal; its prompt is
/// the prompt), but it still funnels through the shared, locked, ESC-sanitized
/// [`inject_and_submit`] so an API delivery serializes against a concurrent
/// composer/`k2 msg` and can never splice keystrokes (D1/D2/D5) or land on an
/// open grok permission gate.
///
/// `wait_ready > 0` uses the same path as `k2 msg` post-wake
/// ([`deliver_post_wake`] via [`inject_raw_into_session_with_profile`]).
/// Zero means inject immediately (no settle/quiescence).
///
/// Default profile = claude-shaped. Prefer
/// [`inject_raw_into_session_with_profile`] when the workspace agent is known
/// (host-sessions always does).
#[allow(dead_code)] // public host-inject API; in-tree callers use with_profile
pub fn inject_raw_into_session(
    session_id: &SessionId,
    payload: &str,
    wait_ready: Duration,
) -> bool {
    inject_raw_into_session_with_profile(
        session_id,
        payload,
        wait_ready,
        &k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE,
    )
}

/// W4 (0.40.30) — [`inject_raw_into_session`] with an explicit
/// [`k2_core::workspace::provider_resume::InjectionProfile`], resolved by
/// the caller through the ONE shared precedence chain
/// (`provider_resume::resolve_injection_profile`: preset-declared
/// `readiness` metadata → static provider table → default).
///
/// **Same shape as `k2 msg` dormant-wake** ([`deliver_post_wake`]): when
/// `wait_ready > 0` this is a thin lookup + call into that path —
/// poll `?2004h` from t=0 (1s after a 0→1 flip) or liar settle, then
/// **screen quiescence** → locked inject. Do not re-implement readiness
/// here; wake-path fixes must land once.
///
/// `wait_ready == 0` injects immediately (legacy "already interactive"
/// shortcut). Prefer a non-zero ceiling for host-session follow-ups so
/// mid-turn / mid-repaint pastes still go through quiescence.
pub fn inject_raw_into_session_with_profile(
    session_id: &SessionId,
    payload: &str,
    wait_ready: Duration,
    profile: &k2_core::workspace::provider_resume::InjectionProfile,
) -> bool {
    let Some(live) = session_lookup::lookup_by_session_id(session_id) else {
        return false;
    };
    if wait_ready.is_zero() {
        return matches!(inject_and_submit(&live, payload), InjectOutcome::Delivered);
    }
    // Single chokepoint with k2 msg post-wake (Issue #9 + d8f80db quiescence).
    matches!(
        deliver_post_wake(&live, payload, wait_ready, profile),
        InjectOutcome::Delivered
    )
}

// ─────────────────────────────────────────────────────────────────────
// Branch implementations
// ─────────────────────────────────────────────────────────────────────

pub(crate) fn lookup_live_for_conversation(conversation_key: &str) -> Option<session_lookup::LiveSession> {
    lookup_live_for_conversation_where(conversation_key, |_| true)
}

/// Project-blind live lookup by conversation key, keeping only sessions
/// `accept` allows. The map scan is `HashMap` order, so with two live
/// sessions that both reference the key the winner is arbitrary.
fn lookup_live_for_conversation_where(
    conversation_key: &str,
    accept: impl Fn(&session_lookup::LiveSession) -> bool,
) -> Option<session_lookup::LiveSession> {
    if let Some(sid) = SessionId::parse(conversation_key) {
        if let Some(live) = session_lookup::lookup_by_session_id(&sid) {
            if accept(&live) {
                return Some(live);
            }
        }
    }
    for (agent_name, live) in session_lookup::snapshot_all() {
        let matches = k2_core::workspace::provider_resume::argv_references_session(
            &live.args(),
            conversation_key,
        ) || agent_name == conversation_key
            || agent_name == format!("tab-{conversation_key}");
        if matches && accept(&live) {
            return Some(live);
        }
    }
    let tab = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, conversation_key)
            .ok()
            .flatten()
    };
    if let Some(tab) = tab {
        if let Some(live) = session_lookup::lookup_any(&tab.agent_name) {
            if accept(&live) {
                return Some(live);
            }
        }
    }
    None
}

/// This workspace's tab rows for a sidecar conversation, newest first:
/// the provider id on the row, or the pane key before one was stamped.
fn project_tab_rows_for_conversation(
    project_id: &str,
    conversation_key: &str,
) -> Vec<k2_core::db::schema::WorkspaceTabSession> {
    let key = conversation_key.trim();
    if key.is_empty() {
        return Vec::new();
    }
    let pane = k2_core::workspace_session_handles::normalize_pane_key(key);
    let tab_key = format!("tab-{pane}");
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::WorkspaceTabSession::list_by_project(&conn, project_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|row| {
            row.session_id.as_deref().map(str::trim) == Some(key)
                || row.pane_group_id == pane
                || row.agent_name == key
                || row.agent_name == tab_key
        })
        .collect()
}

/// True when the live session's cwd belongs to a different registered
/// workspace than `project_id` (longest registered path wins, so a
/// nested workspace is its own owner). Unknown cwd is not foreign.
fn live_owned_by_other_workspace(live: &session_lookup::LiveSession, project_id: &str) -> bool {
    match crate::activity_store::project_for_cwd(&live.cwd()) {
        Some((owner, _)) => owner != project_id,
        None => false,
    }
}

/// Live PTY for a sidecar (`ws/handle`) conversation, scoped to the
/// sidecar's own workspace.
///
/// One provider conversation id can sit in two workspaces: a tab that
/// continued another workspace's Chat (`--fork-session --resume <id>`)
/// is registered under the source id, so its tab row and handle carry
/// the other workspace's pinned conversation. The project-blind lookup
/// then injected a Thread send for `ws/1` into whichever live session the
/// map yielded first, often the other workspace's pinned Chat.
pub(crate) fn lookup_live_for_sidecar(
    project_id: &str,
    conversation_key: &str,
) -> Option<session_lookup::LiveSession> {
    for tab in project_tab_rows_for_conversation(project_id, conversation_key) {
        if let Some(live) = session_lookup::lookup_any(&tab.agent_name) {
            return Some(live);
        }
    }
    lookup_live_for_conversation_where(conversation_key, |live| {
        !live_owned_by_other_workspace(live, project_id)
    })
}

pub(crate) fn tab_row_for_conversation(
    project_id: &str,
    conversation_key: &str,
) -> Option<k2_core::db::schema::WorkspaceTabSession> {
    // This workspace's own row first: the global reverse index below
    // returns the newest row with that provider id in ANY workspace.
    if let Some(row) = project_tab_rows_for_conversation(project_id, conversation_key)
        .into_iter()
        .next()
    {
        return Some(row);
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(row) =
        k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, conversation_key)
            .ok()
            .flatten()
    {
        return Some(row);
    }
    if let Some(row) =
        k2_core::db::schema::WorkspaceTabSession::get(&conn, project_id, conversation_key)
            .ok()
            .flatten()
    {
        return Some(row);
    }
    k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(&conn, project_id, conversation_key)
        .ok()
        .flatten()
        .or_else(|| {
            k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(
                &conn,
                project_id,
                &format!("tab-{conversation_key}"),
            )
            .ok()
            .flatten()
        })
}

fn inject_cell(
    live: &session_lookup::LiveSession,
    text: &str,
    from: Sender<'_>,
    command: &str,
    branch: &str,
) -> MsgResponse {
    let payload = format_sender(from, text, command);
    match inject_and_submit_as(live, &payload, from.is_untrusted()) {
        InjectOutcome::Delivered => MsgResponse::ok(live.session_id().to_string(), branch),
        InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
        InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
        InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
    }
}

#[allow(clippy::too_many_arguments)]
fn attempt_sidecar_delivery(
    project_path: &str,
    handle: &str,
    conversation_key: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
) -> MsgResponse {
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match resolve_project_id(&conn, project_path) {
            Some(p) => p,
            None => return MsgResponse::fail(MsgReason::WorkspaceNotFound),
        }
    };

    if let Some(live) = lookup_live_for_sidecar(&project_id, conversation_key) {
        return inject_cell(&live, text, from, command, "sidecar_live");
    }

    if !wake {
        return MsgResponse::fail(MsgReason::DormantNoWake);
    }

    wake_sidecar_and_fire(
        project_path,
        &project_id,
        conversation_key,
        handle,
        text,
        from,
        command,
        wake_timeout,
    )
}

fn attempt_session_delivery(
    session_id: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake: bool,
    wake_timeout: Duration,
) -> MsgResponse {
    if let Some(live) = lookup_live_for_conversation(session_id) {
        return inject_cell(&live, text, from, command, "session_live");
    }

    // Durable reverse-index: tab row, then canonical workspace_sessions.
    let tab = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, session_id)
            .ok()
            .flatten()
    };
    if let Some(tab) = tab {
        if let Some(live) = session_lookup::lookup_any(&tab.agent_name) {
            return inject_cell(&live, text, from, command, "session_live_tab");
        }
        if !wake {
            return MsgResponse::fail(MsgReason::DormantNoWake);
        }
        let path = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT path FROM projects WHERE id = ?1",
                rusqlite::params![tab.project_id],
                |r| r.get::<_, String>(0),
            )
            .ok()
        };
        let Some(path) = path else {
            return MsgResponse::fail(MsgReason::WorkspaceNotFound);
        };
        return wake_sidecar_and_fire(
            &path,
            &tab.project_id,
            session_id,
            session_id,
            text,
            from,
            command,
            wake_timeout,
        );
    }

    let canonical = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::get_by_session_id(&conn, session_id)
            .ok()
            .flatten()
    };
    if let Some(row) = canonical {
        let path = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.query_row(
                "SELECT path FROM projects WHERE id = ?1",
                rusqlite::params![row.project_id],
                |r| r.get::<_, String>(0),
            )
            .ok()
        };
        let Some(path) = path else {
            return MsgResponse::fail(MsgReason::WorkspaceNotFound);
        };
        return attempt_delivery(&path, text, from, command, wake, wake_timeout);
    }

    if !wake {
        return MsgResponse::fail(MsgReason::DormantNoWake);
    }
    MsgResponse::fail(MsgReason::WorkspaceNotFound)
}

#[allow(clippy::too_many_arguments)]
fn wake_sidecar_and_fire(
    project_path: &str,
    project_id: &str,
    conversation_key: &str,
    _handle: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake_timeout: Duration,
) -> MsgResponse {
    let lock_key = format!("{project_id}:{conversation_key}");
    let wake_lock = wake_lock_for(&lock_key);
    let _wake_guard = wake_lock.lock();

    if let Some(live) = lookup_live_for_sidecar(project_id, conversation_key) {
        return inject_cell(&live, text, from, command, "sidecar_wake_coalesced");
    }

    let Some(tab) = tab_row_for_conversation(project_id, conversation_key) else {
        log_debug!(
            "[msg/wake_sidecar] no durable tab row for key={conversation_key} project={project_id}"
        );
        return MsgResponse::fail(MsgReason::WorkspaceNotFound);
    };
    // Workbench W1: a retired worktree sidecar keeps its Thread, read-only;
    // its worktree is in the Trash, so it never wakes again.
    let retired = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::worktree_task::store::get(&conn, project_id, &tab.pane_group_id).is_some_and(|m| m.retired_at.is_some())
    };
    if retired {
        log_debug!("[msg/wake_sidecar] key={conversation_key} is a retired worktree sidecar; not waking it");
        return MsgResponse::fail(MsgReason::NoAgentMode);
    }

    let saved_cmd = tab.command.clone().filter(|s| !s.is_empty());
    let Some(spawn_command) = saved_cmd else {
        return MsgResponse::fail(MsgReason::NoAgentMode);
    };
    let saved_args: Vec<String> = tab
        .args_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    let wake_cwd = tab
        .cwd
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| project_path.to_string());
    // SC48: the ONE resume helper restart recovery uses. A fresh CLI
    // sidecar's tab row stores `--session-id <cid>`; replaying it would
    // start a new conversation (or be refused) — drop it and resume.
    let relaunch = crate::sidecar_routes::relaunch_plan(
        project_id,
        &tab.pane_group_id,
        &wake_cwd,
        &spawn_command,
        &saved_args,
        tab.session_id.as_deref(),
    );
    let (spawn_args, restart_prompt) = match relaunch {
        Some(r) => (r.args, r.launch_prompt),
        None => (saved_args, None),
    };
    // SC30: a woken sidecar keeps its own label (its Chats name), not the
    // workspace's agent name.
    let wake_label =
        crate::sidecar_routes::sidecar_display_name(&tab.pane_group_id, tab.session_id.as_deref());

    let payload = format_sender(from, text, command);
    // A fresh restart (codex / hermes before discovery) re-points the brief
    // in the same first turn as the message.
    let first_turn = match restart_prompt {
        Some(p) => format!("{p}\n\n{payload}"),
        None => payload.clone(),
    };
    let (spawn_command, spawn_args, wake_timeout, inject_profile, launch_prompt) =
        match crate::wake_headless::test_command() {
            Some(c) => (
                c,
                Vec::new(),
                wake_timeout.min(Duration::from_millis(1500)),
                k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE,
                None,
            ),
            _ => {
                let provider = k2_core::workspace::provider_resume::provider_resume_for_command(
                    &spawn_command,
                )
                .map(|p| p.provider)
                .unwrap_or("");
                (
                    spawn_command,
                    spawn_args,
                    wake_timeout,
                    k2_core::workspace::provider_resume::injection_profile_for_provider(provider),
                    Some(first_turn.clone()),
                )
            }
        };

    let cwd = tab
        .cwd
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| project_path.to_string());
    let wake_start = std::time::Instant::now();
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: tab.agent_name.clone(),
        project_id: Some(project_id.to_string()),
        cwd,
        command: Some(spawn_command),
        args: Some(spawn_args),
        cols: 120,
        rows: 38,
        // Register under the tab/API map key — never the canonical slot.
        canonical_key: Some(tab.agent_name.clone()),
        env: HashMap::new(),
        env_owner_set: false,
        launch_prompt,
        label: wake_label,
    }) {
        Ok(o) => o,
        Err(e) => {
            log_debug!("[msg/wake_sidecar] spawn failed: {e}");
            return MsgResponse::fail(spawn_fail_reason(&e));
        }
    };
    let target_id = outcome.session_id.to_string();
    let live = match session_lookup::lookup_by_session_id(&outcome.session_id) {
        Some(l) => l,
        None => {
            log_debug!("[msg/wake_sidecar] post-spawn lookup miss for session={target_id}");
            return MsgResponse::fail(MsgReason::SpawnFailed);
        }
    };
    if outcome.launch_prompt_attached && !outcome.reused {
        log_debug!("[msg/wake] launch-param attached; skipping post-spawn inject");
        crate::active_reaper::note_workspace_need(project_id);
        return MsgResponse::ok_woke(
            target_id,
            "sidecar_wake",
            wake_start.elapsed().as_millis() as u64,
        );
    }
    match deliver_post_wake_as(&live, &first_turn, wake_timeout, &inject_profile, from.is_untrusted()) {
        InjectOutcome::Delivered => MsgResponse::ok_woke(
            target_id,
            "sidecar_wake",
            wake_start.elapsed().as_millis() as u64,
        ),
        InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
        InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
        InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
    }
}

#[allow(clippy::too_many_arguments)]
fn inject_live(
    live: &session_lookup::LiveSession,
    text: &str,
    from: Sender<'_>,
    command: &str,
    branch: &str,
    project_id: &str,
) -> MsgResponse {
    let payload = format_sender(from, text, command);
    match inject_and_submit_as(live, &payload, from.is_untrusted()) {
        InjectOutcome::Delivered => {}
        InjectOutcome::PtyDied => {
            log_debug!("[msg/inject_live] PTY died during injection — pty_died");
            return MsgResponse::fail(MsgReason::PtyDied);
        }
        InjectOutcome::Stalled => {
            log_debug!("[msg/inject_live] injection lock stalled — pty_stalled");
            return MsgResponse::fail(MsgReason::PtyStalled);
        }
        InjectOutcome::GateHold => {
            log_debug!("[msg/inject_live] grok permission gate open — hitl_gate_open");
            return MsgResponse::fail(MsgReason::HitlGateOpen);
        }
    }

    let target_id = live.session_id().to_string();

    // Re-stamp `active_terminal_id` so subsequent calls fast-path
    // through Branch 1 directly. Idempotent if it already pointed here.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
    }

    crate::active_reaper::note_workspace_need(project_id);
    MsgResponse::ok(target_id, branch)
}

/// Issue #9 — re-resolve a now-live session for `project_id` AFTER taking
/// the wake lock. A concurrent waker may have just spawned the canonical
/// session; if so we inject into THAT rather than spawning a duplicate.
/// Mirrors `attempt_delivery`'s Branch-1 (active_terminal_id) + Branch-1b
/// (argv-scan) live lookups, but read-only (no stale-stamp clearing).
fn relookup_live(
    project_id: &str,
    saved_session: Option<&str>,
) -> Option<session_lookup::LiveSession> {
    let row = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::get(&conn, project_id).ok().flatten()
    };
    if let Some(active_tid) = row
        .as_ref()
        .and_then(|r| r.active_terminal_id.clone())
        .filter(|s| !s.is_empty())
    {
        if let Some(sid) = SessionId::parse(&active_tid) {
            if let Some(live) = session_lookup::lookup_by_session_id(&sid) {
                return Some(live);
            }
        }
    }
    if let Some(saved_sid) = saved_session {
        for (_n, live) in session_lookup::snapshot_all() {
            if k2_core::workspace::provider_resume::argv_references_session(&live.args(), saved_sid)
            {
                return Some(live);
            }
        }
    }
    None
}

/// Issue #9 — wait until a freshly woken session is READY, then inject the
/// body through the SHARED per-session lock (D7).
///
/// Slice 5: readiness is PER-PROVIDER via
/// [`k2_core::workspace::provider_resume::InjectionProfile`]:
/// - `ready_via_bracketed_paste == true` (claude/grok/cursor + the
///   unknown-provider default): poll `bracketed_paste_active()` from
///   t=0 (no pre-poll `post_spawn_settle`). Extra [`POST_READY_SETTLE`]
///   (1s) only after a real 0→1 `?2004h` flip (fresh spawn / dormant
///   wake). Already-on (live follow-up) skips the 1s. Timeout without
///   a flip: best-effort inject, no extra 1s.
/// - `ready_via_bracketed_paste == false` (codex/gemini/pi/hermes):
///   ?2004h LIES for these providers (set before real readiness, or
///   toggled per repaint), so readiness is the profile's conservative
///   settle floor alone — [`WAKE_MIN_SETTLE`] is the floor (hermes ~7s
///   first-message; codex/gemini 2s; pi 1.5s). Do not poll `?2004h`.
///
/// Both paths then wait for screen quiescence before
/// [`inject_and_submit`]. Returns the [`InjectOutcome`] so the caller
/// maps it to the canonical response.
fn deliver_post_wake(
    live: &session_lookup::LiveSession,
    payload: &str,
    wake_timeout: Duration,
    profile: &k2_core::workspace::provider_resume::InjectionProfile,
) -> InjectOutcome {
    deliver_post_wake_as(live, payload, wake_timeout, profile, false)
}

/// [`deliver_post_wake`] with the sender's type ([`Sender::is_untrusted`]).
fn deliver_post_wake_as(
    live: &session_lookup::LiveSession,
    payload: &str,
    wake_timeout: Duration,
    profile: &k2_core::workspace::provider_resume::InjectionProfile,
    untrusted: bool,
) -> InjectOutcome {
    let start = std::time::Instant::now();
    if profile.ready_via_bracketed_paste {
        // Honest / polling: poll from t=0. Extra 1s only on a real 0→1
        // `?2004h` flip (fresh spawn / dormant wake). Already-on skips
        // it so live API follow-ups are not taxed. Ceiling without a
        // flip: no extra 1s, best-effort inject.
        let ready = wait_for_bracketed_paste(live, start, wake_timeout);
        if ready == PasteReady::PtyDied {
            return InjectOutcome::PtyDied;
        }
        let settle = ready.post_ready_settle();
        if !settle.is_zero() {
            std::thread::sleep(settle);
            if !live.is_child_alive() {
                return InjectOutcome::PtyDied;
            }
        }
    } else {
        // Liar / non-polling: the settle floor IS the readiness wait.
        std::thread::sleep(profile.post_spawn_settle.max(WAKE_MIN_SETTLE));
        if !live.is_child_alive() {
            return InjectOutcome::PtyDied;
        }
    }

    // Quiescence gate. The bracketed-paste ready signal (`?2004h`) is set
    // by claude WHILE it is still finalizing a resumed transcript's render:
    // a resume of a large conversation flips it early, then repaints the
    // composer and discards any text pasted into that pre-final frame — the
    // message was reported Delivered but silently dropped (dormant-wake
    // delivery black hole, confirmed via grid capture: 0/45 frames + never
    // in the recipient's transcript). Before injecting, wait for the woken
    // TUI's visible grid to stop changing, so the replay + repaint are both
    // done and the paste lands in the same settled, idle input box that live
    // delivery injects into successfully. A child that exits while we wait
    // is a real `pty_died`; a stable screen OR the wake_timeout ceiling both
    // proceed to inject.
    if let InjectOutcome::PtyDied = wait_for_screen_quiescence(live, start, wake_timeout) {
        return InjectOutcome::PtyDied;
    }

    // D7: post-wake delivery funnels through the SAME locked
    // `inject_and_submit` as live sends, so the just-woken session's
    // injection serializes against a concurrent human composer.
    inject_and_submit_as(live, payload, untrusted)
}

/// How the polling readiness wait in [`deliver_post_wake`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PasteReady {
    /// `?2004h` was already on (a live follow-up).
    AlreadyOn,
    /// A real 0→1 `?2004h` flip (fresh spawn / dormant wake).
    Flipped,
    /// The `wake_timeout` ceiling hit without a flip: best-effort inject.
    TimedOut,
    /// The child exited while we waited.
    PtyDied,
}

impl PasteReady {
    /// Extra settle before injecting: [`POST_READY_SETTLE`] only after a
    /// real flip; never for already-on or a timeout.
    fn post_ready_settle(self) -> Duration {
        match self {
            PasteReady::Flipped => POST_READY_SETTLE,
            PasteReady::AlreadyOn | PasteReady::TimedOut | PasteReady::PtyDied => Duration::ZERO,
        }
    }
}

/// Poll for the TUI's bracketed-paste ready signal (claude/grok/cursor set
/// it once the input box is drawn; the same signal the injector relies on
/// for framing) until it flips, the child dies, or `wake_timeout` (from
/// `start`) passes.
fn wait_for_bracketed_paste(
    live: &session_lookup::LiveSession,
    start: std::time::Instant,
    wake_timeout: Duration,
) -> PasteReady {
    if live.bracketed_paste_active() {
        return PasteReady::AlreadyOn;
    }
    loop {
        if !live.is_child_alive() {
            return PasteReady::PtyDied;
        }
        if live.bracketed_paste_active() {
            return PasteReady::Flipped;
        }
        if start.elapsed() >= wake_timeout {
            // Best-effort: timed out waiting for the ready signal — inject
            // anyway (bounded, never blocks forever) rather than dropping
            // the message.
            log_debug!(
                "[msg/wake] session={} readiness wait hit {}ms ceiling — injecting best-effort",
                live.session_id(),
                wake_timeout.as_millis()
            );
            return PasteReady::TimedOut;
        }
        std::thread::sleep(WAKE_POLL_INTERVAL);
    }
}

/// Block until the just-woken TUI's visible grid stops changing — the
/// resumed-transcript stream has ended AND the composer has repainted —
/// so a subsequent inject lands in a stable idle input box rather than a
/// pre-final frame a repaint will wipe (see [`deliver_post_wake`]).
///
/// Returns [`InjectOutcome::PtyDied`] if the child exits while waiting.
/// Otherwise returns [`InjectOutcome::Delivered`] purely as a "safe to
/// inject now" signal (screen settled, or the `wake_timeout` ceiling hit) —
/// the caller performs the real injection next in either case.
///
/// An idle claude composer's grid is static (verified: 45 identical frames
/// across ~14s of idle), so the stability window reliably fires shortly
/// after the replay ends; a still-streaming screen keeps resetting it.
fn wait_for_screen_quiescence(
    live: &session_lookup::LiveSession,
    start: std::time::Instant,
    wake_timeout: Duration,
) -> InjectOutcome {
    let mut last: Option<Vec<String>> = None;
    let mut stable_since: Option<std::time::Instant> = None;
    loop {
        if !live.is_child_alive() {
            return InjectOutcome::PtyDied;
        }
        let rows = live.visible_text_rows();
        let now = std::time::Instant::now();
        match &last {
            // Unchanged since the last poll — has it been quiet long enough?
            Some(prev) if *prev == rows => {
                let quiet_for = stable_since
                    .map(|s| now.duration_since(s))
                    .unwrap_or_default();
                if quiet_for >= WAKE_QUIESCE_STABLE {
                    return InjectOutcome::Delivered;
                }
            }
            // Changed (still streaming/repainting) — reset the stability clock.
            _ => {
                stable_since = Some(now);
                last = Some(rows);
            }
        }
        if start.elapsed() >= wake_timeout {
            log_debug!(
                "[msg/wake] session={} quiescence wait hit {}ms ceiling — injecting best-effort",
                live.session_id(),
                wake_timeout.as_millis()
            );
            return InjectOutcome::Delivered;
        }
        std::thread::sleep(WAKE_QUIESCE_POLL);
    }
}

/// Issue #9 wake (branches 2+3), Slice-3b rewired. Spawns the
/// workspace's canonical agent session — resume or fresh — and
/// synchronously delivers the body once the TUI is ready.
///
/// **Which binary + argv (the wake-wrong-binary fix, research dragon
/// #3):** everything comes from the canonical resume resolver
/// (`resume_chat::resolve_resume_chat_args`), the same seam the pinned
/// chat tab's ensure path uses:
/// - an existing canonical session resumes with the agent its stored
///   `workspace_sessions.harness` names (a grok workspace wakes `grok`,
///   never `claude`), speaking that provider's resume grammar via the
///   `ProviderResume` adapter (`--resume`, pi `--session`, codex
///   `resume <id>`);
/// - a fresh wake follows the workspace/global default agent: premint
///   providers (claude/grok) pin `--session-id <new-uuid>`, self-minting
///   providers spawn bare and the discovered id is adopted post-hoc
///   (`defer_adopt_discovered_session`), unknown providers spawn bare
///   with no invented flags;
/// - claude argv stays byte-identical to the pre-3b hardcode
///   (`--dangerously-skip-permissions --resume <sid>` /
///   `--session-id <new>`).
///
/// The resolver runs INSIDE the per-project wake lock so a concurrent
/// waker can never double-mint a session identity; the post-lock
/// relookup still coalesces onto a session a first waker just spawned.
#[allow(clippy::too_many_arguments)]
fn wake_and_fire(
    project_path: &str,
    project_id: &str,
    agent_name: &str,
    text: &str,
    from: Sender<'_>,
    command: &str,
    wake_timeout: Duration,
) -> MsgResponse {
    // Issue #9 — serialize the wake on `project_id`. Two callers
    // `talk`/`msg --wake`-ing the same dormant peer must not each spawn a
    // duplicate session. The holder spawns + waits-for-ready + injects; a
    // waiter blocks here, then re-checks for the session the first waker
    // just spawned (below) and injects into THAT.
    let wake_lock = wake_lock_for(project_id);
    let _wake_guard = wake_lock.lock();

    // Post-lock re-check: did a concurrent waker already wake the peer? If
    // a live canonical session now exists, deliver into it (no duplicate
    // spawn). `woke: false` because THIS call didn't pay the wake cost.
    let saved_session = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::get(&conn, project_id)
            .ok()
            .flatten()
            .and_then(|r| r.session_id)
            .filter(|s| !s.is_empty())
    };
    if let Some(live) = relookup_live(project_id, saved_session.as_deref()) {
        let payload = format_sender(from, text, command);
        return match inject_and_submit_as(&live, &payload, from.is_untrusted()) {
            InjectOutcome::Delivered => {
                crate::active_reaper::note_workspace_need(project_id);
                MsgResponse::ok(live.session_id().to_string(), "wake_coalesced")
            }
            InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
            InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
            InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
        };
    }

    // Resolve command + argv + session identity (see fn doc). Held
    // inside the wake lock so the identity writes (premint/converge)
    // are single-flight per workspace.
    let resolved = match k2_core::workspace::resume_chat::resolve_resume_chat_args(project_path) {
        Ok(r) => r,
        Err(e) => {
            log_debug!("[msg/wake] resume resolve failed for {project_path}: {e}");
            return MsgResponse::fail(if k2_core::workspace::resume_chat::is_main_chat_missing(&e) {
                MsgReason::MainChatMissing
            } else {
                MsgReason::SpawnFailed
            });
        }
    };
    let branch = if resolved.resumed_existing {
        "resume_and_fire"
    } else {
        "fresh_fire"
    };

    // Test seam — mirrors `wake_headless::spawn_wake_headless`:
    // substitute a benign command (e.g. `cat`) so integration tests
    // exercise the wake path without a real agent binary. `cat` never
    // advertises bracketed-paste readiness, so the readiness ceiling is
    // clamped to keep tests fast. Production never sets this.
    //
    // Slice 5: the injection profile rides the same seam — real wakes
    // speak the resolved provider's readiness dialect; the test
    // override keeps the claude-shaped default (it spawns `cat`, not
    // the provider).
    let payload = format_sender(from, text, command);
    let (spawn_command, spawn_args, wake_timeout, inject_profile, launch_prompt) =
        match crate::wake_headless::test_command() {
            Some(c) => (
                c,
                Vec::new(),
                wake_timeout.min(Duration::from_millis(1500)),
                k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE,
                None,
            ),
            _ => (
                resolved.command.clone(),
                resolved.args.clone(),
                wake_timeout,
                // W4: the ONE shared precedence chain — the spawning
                // preset's declared readiness metadata wins over the
                // static provider table (which keeps its own default
                // for agents neither has studied).
                k2_core::workspace::provider_resume::resolve_injection_profile(
                    resolved.readiness.as_deref(),
                    &resolved.provider,
                ),
                Some(payload.clone()),
            ),
        };

    let wake_start = std::time::Instant::now();
    let outcome = match spawn_agent_session_v2_blocking(SpawnWorkspaceSessionRequest {
        agent_name: agent_name.to_string(),
        project_id: Some(project_id.to_string()),
        cwd: project_path.to_string(),
        command: Some(spawn_command),
        args: Some(spawn_args),
        cols: 120,
        rows: 38,
        canonical_key: None,
        // W2: the spawning preset's migration-0070 env (empty under the
        // test override). Values never logged.
        env: resolved
            .env
            .clone()
            .map(|m| m.into_iter().collect())
            .unwrap_or_default(),
        // the preset's env is owner-set (R8)
        env_owner_set: true,
        launch_prompt,
        label: None,
    }) {
        Ok(o) => o,
        Err(e) => {
            log_debug!("[msg/wake_and_fire] spawn failed: {e}");
            return MsgResponse::fail(spawn_fail_reason(&e));
        }
    };
    let target_id = outcome.session_id.to_string();

    // Upsert the workspace_sessions row + stamp session_id. Pre-0.37.5
    // history: see git log for `0.37.5 Phase D` if you want the
    // archaeology. The resolver already persisted the identity for the
    // premint/converge cases; the stamp below keeps the resume case's
    // pre-3b parity (idempotent re-write of the same id).
    let canonical_terminal_id = format!("agent-chat:{project_id}");
    let _ = k2_core::workspace::session::k2_agents_lock(
        project_path.to_string(),
        agent_name.to_string(),
        Some(canonical_terminal_id),
        Some("system".to_string()),
    );
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = WorkspaceSession::save_active_terminal_id(&conn, project_id, &target_id);
        if !resolved.resume_session.is_empty() {
            let _ =
                WorkspaceSession::update_session_id(&conn, project_id, &resolved.resume_session);
        }
    }

    // Self-minting provider (pi/codex/gemini/cursor): adopt the session
    // id the agent creates on disk a beat after spawn, stamping
    // `workspace_sessions.session_id` + `harness` (Slice 3b).
    if resolved.pending_session_discovery {
        k2_core::workspace::provider_resume::defer_adopt_discovered_session(
            resolved.provider.clone(),
            project_path.to_string(),
        );
    }

    log_debug!(
        "[daemon/workspace-msg] {} session={} agent={} from={}",
        branch,
        target_id,
        agent_name,
        from.name()
    );

    // Issue #9 — SYNCHRONOUS wake-then-deliver. The recipient was dormant;
    // we wait until the freshly spawned TUI is READY (bounded by
    // `wake_timeout`) and inject the body BEFORE returning, so the caller
    // (a) knows delivery actually happened and (b) gets the wake duration
    // to attribute the latency. The wait runs while we still hold the
    // per-project wake lock, so a concurrent waker keeps coalescing onto
    // this woken session instead of spawning its own. The body goes
    // through the same locked `inject_and_submit` as live delivery (D7) —
    // paste framing + insurance Enter + per-session serialization.
    let session = session_lookup::lookup_by_session_id(&outcome.session_id);
    let Some(live) = session else {
        log_debug!(
            "[daemon/workspace-msg] post-spawn lookup miss for session={} — body not delivered",
            target_id
        );
        return MsgResponse::fail(MsgReason::SpawnFailed);
    };

    // Slice 5 documented-deferred (hard-safety rule b): grok's FIRST
    // send in a fresh cwd is intercepted by a project-directory picker
    // ("Run Grok Build in a project directory?" — study modal #1). Its
    // default answer, "use current directory", is SAFE, and the
    // injector's submit-CR + insurance-CR sequence answers it with that
    // default in practice — but K2 does not yet explicitly detect the
    // picker and re-deliver the body afterwards, so a first-ever wake
    // into a brand-new grok workspace may leave the message typed into
    // the picker rather than the composer. Deliberately deferred (not
    // half-built): detection needs a live-grok validation pass; the
    // permission GATE (rule a, the dangerous one) is enforced above in
    // the injection chokepoint.
    if outcome.launch_prompt_attached && !outcome.reused {
        let wake_ms = wake_start.elapsed().as_millis() as u64;
        log_debug!("[msg/wake] launch-param attached; skipping post-spawn inject");
        crate::active_reaper::note_workspace_need(project_id);
        return MsgResponse::ok_woke(target_id, branch, wake_ms);
    }

    let _ = Path::new(project_path);
    match deliver_post_wake_as(&live, &payload, wake_timeout, &inject_profile, from.is_untrusted()) {
        InjectOutcome::Delivered => {
            let wake_ms = wake_start.elapsed().as_millis() as u64;
            log_debug!(
                "[daemon/workspace-msg] {} woke session={} in {}ms agent={} from={}",
                branch,
                target_id,
                wake_ms,
                agent_name,
                from.name()
            );
            crate::active_reaper::note_workspace_need(project_id);
            MsgResponse::ok_woke(target_id, branch, wake_ms)
        }
        InjectOutcome::PtyDied => {
            log_debug!(
                "[daemon/workspace-msg] PTY died during post-wake inject for session={}",
                target_id
            );
            MsgResponse::fail(MsgReason::PtyDied)
        }
        InjectOutcome::Stalled => {
            log_debug!(
                "[daemon/workspace-msg] injection lock stalled during post-wake inject for session={}",
                target_id
            );
            MsgResponse::fail(MsgReason::PtyStalled)
        }
        InjectOutcome::GateHold => {
            log_debug!(
                "[daemon/workspace-msg] grok permission gate open during post-wake inject for session={}",
                target_id
            );
            MsgResponse::fail(MsgReason::HitlGateOpen)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[path = "workspace_msg_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workspace_msg_voice_chat_inject_tests.rs"]
mod voice_chat_inject_tests;
