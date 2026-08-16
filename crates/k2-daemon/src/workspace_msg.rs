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

use k2_core::workspace::agent_identity::resolve_project_id;
use k2_core::db::schema::{Project, WorkspaceSession};
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
}

impl MsgReason {
    fn code(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => "workspace_not_found",
            Self::NoAgentMode => "no_agent_mode",
            Self::SpawnFailed => "spawn_failed",
            Self::PtyDied => "pty_died",
            Self::PtyStalled => "pty_stalled",
            Self::DormantNoWake => "dormant_no_wake",
            Self::HitlGateOpen => "hitl_gate_open",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::WorkspaceNotFound => {
                "Run `k2so connections list` to see available workspaces."
            }
            Self::NoAgentMode => {
                "Workspace has no agent. Use `k2 work send` to queue, or `k2 mode custom` to set up an agent."
            }
            Self::SpawnFailed => {
                "Spawn failed. Verify the workspace's agent CLI (e.g. `claude`) is on PATH for the daemon."
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
            Self::WorkspaceNotFound | Self::NoAgentMode | Self::DormantNoWake
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

    fn fail(reason: MsgReason) -> Self {
        Self {
            success: false,
            target_session_id: None,
            attempts: 1,
            reason: Some(reason.code().to_string()),
            hint: Some(reason.hint().to_string()),
            woke: false,
            wake_ms: None,
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
        WorkspaceResolve::Ambiguous { .. } | WorkspaceResolve::Miss => None,
    }
}

/// Local resolve with AMBIG surface (D19 / §9.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceResolve {
    Found(String),
    Ambiguous { handles: Vec<String> },
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
        k2_core::workspace::handle::WorkspaceTokenResolve::Miss => WorkspaceResolve::Miss,
    }
}

/// First-arg target for `k2 msg` / `talk` / `read` (D12).
///
/// Federation `agent::host` is split in the CLI — not parsed here.
/// `sales-reviewer` is a workspace name (or fail), never a sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MsgTarget {
    WorkspaceCanonical { path: String },
    Sidecar {
        path: String,
        handle: String,
        conversation_key: String,
    },
    /// UUID that matched a live or durable session (provider or PTY id).
    Session { session_id: String },
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

    if let Some((ws, handle)) = k2_core::workspace_session_handles::split_workspace_handle(token)
    {
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
        let conversation_key = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace_session_handles::resolve_handle(&conn, &project_id, handle).ok()
        }?;
        return Some(MsgTarget::Sidecar {
            path,
            handle: handle.to_string(),
            conversation_key,
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
    let sender = humanize_chat_from(from, "external");
    let command = command.trim();
    if command.is_empty() {
        format!("[from {sender}] {text}")
    } else {
        format!("{command} [from {sender}] {text}")
    }
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
/// - It has no `command` slash-prefix branch (the composer never sends
///   slash-commands; that surface is `k2 msg`/`--command` only).
pub fn format_message_user(from: &str, text: &str) -> String {
    let sender = humanize_chat_from(from, "owner");
    format!("[from {sender}] {text}")
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
/// attributed `from` from the canonical, daemon-side app_settings
/// (`owner_display_name`), falling back to `"owner"` when unset/blank.
///
/// **D3 holds:** the name is read ONLY from server-side settings — NEVER
/// from the request body — so a client cannot spoof the attribution.
pub fn resolve_owner_from() -> String {
    owner_from_or_default(k2_core::app_settings::load().owner_display_name.as_deref())
}

/// Composer 1a (D2) — max time a waiting injector blocks for the
/// per-session lock before giving up with a truthful `pty_stalled`.
/// Generously exceeds a single injection's ~520ms of timed sleeps so
/// legitimately-serialized sends (composer + `k2 msg`) all land in
/// order, while a truly stuck holder still releases the waiter instead
/// of pinning it forever.
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

/// Issue #9 — minimum settle before the post-wake inject, even if the
/// readiness signal flips early. Guards against injecting into a
/// half-drawn first frame.
const WAKE_MIN_SETTLE: Duration = Duration::from_millis(400);

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
    // Resolve once (D12). Unknown token is permanent; surface immediately.
    let target = match resolve_msg_target(workspace_token) {
        Some(t) => t,
        None => {
            let suggest_token =
                k2_core::workspace_session_handles::split_workspace_handle(workspace_token)
                    .map(|(ws, _)| ws)
                    .unwrap_or(workspace_token);
            if let WorkspaceResolve::Ambiguous { handles } =
                resolve_workspace_detailed(suggest_token)
            {
                let mut resp = MsgResponse::fail(MsgReason::WorkspaceNotFound);
                resp.hint = Some(format!(
                    "AMBIG: '{suggest_token}' matches multiple workspaces. Use a unique handle: {}",
                    handles.join(", ")
                ));
                return resp;
            }
            let mut resp = MsgResponse::fail(MsgReason::WorkspaceNotFound);
            let suggestion = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::connections::suggest_project_name(&conn, suggest_token)
            };
            if workspace_token.contains('/') && !workspace_token.starts_with('/') {
                resp.hint = Some(format!(
                    "Unknown workspace/handle '{workspace_token}'. Use `k2 msg <workspace>` for the primary or `k2 msg <workspace>/<handle>` for a sidecar (`k2 sessions live`). Hyphenated names like `sales-reviewer` are workspaces, not sidecars."
                ));
            } else if let Some(s) = suggestion {
                resp.hint = Some(format!(
                    "Unknown workspace '{workspace_token}' — did you mean '{s}'? Run `k2so connections list` to see available workspaces."
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
            MsgTarget::Session { session_id } => attempt_session_delivery(
                session_id,
                text,
                from,
                command,
                wake,
                wake_timeout,
            ),
        };
        result.attempts = attempt;

        if result.success {
            return result;
        }

        // Permanent reasons short-circuit — waiting won't fix them.
        // `dormant_no_wake` is permanent for THIS call: re-firing without
        // a wake request will keep returning it (#9).
        if let Some(reason) = result.reason.as_deref() {
            let permanent = matches!(
                reason,
                "workspace_not_found" | "no_agent_mode" | "dormant_no_wake"
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
    from: &str,
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
                return inject_live(&live, text, from, command, "active_terminal_id", &project_id);
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
            if k2_core::workspace::provider_resume::argv_references_session(
                &live.args(),
                saved_sid,
            ) {
                return inject_live(&live, text, from, command, "argv_scan", &project_id);
            }
        }
    }

    // ── Issue #9: classify the no-live-session state BEFORE the spawn
    // branches, so the three states the ticket calls out are distinct:
    //   • no agent configured            → no_agent_mode (un-wakeable;
    //     clear remedy `k2 mode custom` / register). Do NOT spawn.
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
    rows.iter()
        .rev()
        .take(GROK_GATE_SCAN_ROWS)
        .any(|row| {
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
fn inject_and_submit(
    live: &session_lookup::LiveSession,
    payload: &str,
) -> InjectOutcome {
    inject_and_submit_with_timeout(live, payload, INJECT_LOCK_TIMEOUT)
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
    inject_framed_locked(live, payload)
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
fn inject_framed_locked(
    live: &session_lookup::LiveSession,
    payload: &str,
) -> InjectOutcome {
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
    // D5 — strip raw ESC BEFORE framing so an embedded `ESC[201~` can't
    // close the paste early and splice live keystrokes.
    let clean = sanitize_inject_text(payload);
    let body: Vec<u8> = if live.bracketed_paste_active() {
        let mut b = Vec::with_capacity(clean.len() + 12);
        b.extend_from_slice(b"\x1b[200~");
        b.extend_from_slice(clean.as_bytes());
        b.extend_from_slice(b"\x1b[201~");
        b
    } else {
        clean.into_bytes()
    };
    if live.write(&body).is_err() {
        return InjectOutcome::PtyDied;
    }
    // Let the TUI ingest + render the body before the submit keystroke.
    std::thread::sleep(Duration::from_millis(150));

    // Submit. The second CR after a settle is the latency insurance: if
    // the first landed before the paste finished ingesting, this one
    // submits it; if the first already submitted, this hits an empty
    // input box and no-ops.
    if live.write(b"\r").is_err() {
        return InjectOutcome::PtyDied;
    }
    std::thread::sleep(Duration::from_millis(250));
    if !live.is_child_alive() {
        return InjectOutcome::PtyDied;
    }
    let _ = live.write(b"\r");
    std::thread::sleep(Duration::from_millis(120));
    if !live.is_child_alive() {
        return InjectOutcome::PtyDied;
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
pub fn send_message_to_session(session_id: &str, from: &str, text: &str) -> MsgResponse {
    // M1 — resolve to a LIVE session; never spawn.
    let sid = match SessionId::parse(session_id) {
        Some(s) => s,
        None => return MsgResponse::fail(MsgReason::PtyDied),
    };
    let live = match session_lookup::lookup_by_session_id(&sid) {
        Some(l) => l,
        None => return MsgResponse::fail(MsgReason::PtyDied),
    };

    let payload = format_message_user(from, text);
    match inject_and_submit(&live, &payload) {
        InjectOutcome::Delivered => {
            MsgResponse::ok(live.session_id().to_string(), "send_message")
        }
        InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
        InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
        InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
    }
}

/// Persist compose-bar send history. Called ONLY after a successful
/// `POST /cli/terminal/send-message`. Tickets go through
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
/// min settle → readiness dialect → **screen quiescence** → locked
/// inject. Do not re-implement readiness here; wake-path fixes (e.g.
/// quiescence for early `?2004h`) must land once.
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

fn lookup_live_for_conversation(conversation_key: &str) -> Option<session_lookup::LiveSession> {
    if let Some(sid) = SessionId::parse(conversation_key) {
        if let Some(live) = session_lookup::lookup_by_session_id(&sid) {
            return Some(live);
        }
    }
    for (agent_name, live) in session_lookup::snapshot_all() {
        if k2_core::workspace::provider_resume::argv_references_session(
            &live.args(),
            conversation_key,
        ) {
            return Some(live);
        }
        if agent_name == conversation_key
            || agent_name == format!("tab-{conversation_key}")
        {
            return Some(live);
        }
    }
    let tab = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, conversation_key)
            .ok()
            .flatten()
            .or_else(|| {
                // conversation_key may still be pane_group_id
                let rows = k2_core::db::schema::WorkspaceTabSession::list_by_project(
                    &conn,
                    // unknown project — scan is too wide; skip
                    "",
                )
                .ok();
                let _ = rows;
                None
            })
    };
    if let Some(tab) = tab {
        if let Some(live) = session_lookup::lookup_any(&tab.agent_name) {
            return Some(live);
        }
    }
    None
}

fn tab_row_for_conversation(
    project_id: &str,
    conversation_key: &str,
) -> Option<k2_core::db::schema::WorkspaceTabSession> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Some(row) =
        k2_core::db::schema::WorkspaceTabSession::get_by_session_id(&conn, conversation_key)
            .ok()
            .flatten()
    {
        return Some(row);
    }
    if let Some(row) = k2_core::db::schema::WorkspaceTabSession::get(
        &conn,
        project_id,
        conversation_key,
    )
    .ok()
    .flatten()
    {
        return Some(row);
    }
    k2_core::db::schema::WorkspaceTabSession::get_by_agent_name(
        &conn,
        project_id,
        conversation_key,
    )
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
    from: &str,
    command: &str,
    branch: &str,
) -> MsgResponse {
    let payload = format_message(from, text, command);
    match inject_and_submit(live, &payload) {
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
    from: &str,
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

    if let Some(live) = lookup_live_for_conversation(conversation_key) {
        return inject_cell(&live, text, from, command, "sidecar_live");
    }
    // Also try the tab's agent_name if the reverse index has a row.
    if let Some(tab) = tab_row_for_conversation(&project_id, conversation_key) {
        if let Some(live) = session_lookup::lookup_any(&tab.agent_name) {
            return inject_cell(&live, text, from, command, "sidecar_live_tab");
        }
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
    from: &str,
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
    from: &str,
    command: &str,
    wake_timeout: Duration,
) -> MsgResponse {
    let lock_key = format!("{project_id}:{conversation_key}");
    let wake_lock = wake_lock_for(&lock_key);
    let _wake_guard = wake_lock.lock();

    if let Some(live) = lookup_live_for_conversation(conversation_key) {
        return inject_cell(&live, text, from, command, "sidecar_wake_coalesced");
    }

    let Some(tab) = tab_row_for_conversation(project_id, conversation_key) else {
        log_debug!(
            "[msg/wake_sidecar] no durable tab row for key={conversation_key} project={project_id}"
        );
        return MsgResponse::fail(MsgReason::WorkspaceNotFound);
    };

    let saved_cmd = tab.command.clone().filter(|s| !s.is_empty());
    let Some(spawn_command) = saved_cmd else {
        return MsgResponse::fail(MsgReason::NoAgentMode);
    };
    let mut spawn_args: Vec<String> = tab
        .args_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();
    if let Some(sid) = tab.session_id.as_deref().filter(|s| !s.is_empty()) {
        if let Some(adapter) =
            k2_core::workspace::provider_resume::provider_resume_for_command(&spawn_command)
        {
            if !adapter.argv_carries_session_identity(&spawn_args) {
                spawn_args = adapter.resume_args(&spawn_args, sid);
            }
        }
    }

    let (spawn_command, spawn_args, wake_timeout) =
        match std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND") {
            Ok(c) if !c.is_empty() => (
                c,
                Vec::new(),
                wake_timeout.min(Duration::from_millis(1500)),
            ),
            _ => (spawn_command, spawn_args, wake_timeout),
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
    }) {
        Ok(o) => o,
        Err(e) => {
            log_debug!("[msg/wake_sidecar] spawn failed: {e}");
            return MsgResponse::fail(MsgReason::SpawnFailed);
        }
    };
    let target_id = outcome.session_id.to_string();
    let live = match session_lookup::lookup_by_session_id(&outcome.session_id) {
        Some(l) => l,
        None => {
            log_debug!(
                "[msg/wake_sidecar] post-spawn lookup miss for session={target_id}"
            );
            return MsgResponse::fail(MsgReason::SpawnFailed);
        }
    };
    let payload = format_message(from, text, command);
    match deliver_post_wake(
        &live,
        &payload,
        wake_timeout,
        &k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE,
    ) {
        InjectOutcome::Delivered => {
            MsgResponse::ok_woke(target_id, "sidecar_wake", wake_start.elapsed().as_millis() as u64)
        }
        InjectOutcome::PtyDied => MsgResponse::fail(MsgReason::PtyDied),
        InjectOutcome::Stalled => MsgResponse::fail(MsgReason::PtyStalled),
        InjectOutcome::GateHold => MsgResponse::fail(MsgReason::HitlGateOpen),
    }
}

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
    match inject_and_submit(live, &payload) {
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

    MsgResponse::ok(target_id, branch)
}

/// Issue #9 — re-resolve a now-live session for `project_id` AFTER taking
/// the wake lock. A concurrent waker may have just spawned the canonical
/// session; if so we inject into THAT rather than spawning a duplicate.
/// Mirrors `attempt_delivery`'s Branch-1 (active_terminal_id) + Branch-1b
/// (argv-scan) live lookups, but read-only (no stale-stamp clearing).
fn relookup_live(project_id: &str, saved_session: Option<&str>) -> Option<session_lookup::LiveSession> {
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
            if k2_core::workspace::provider_resume::argv_references_session(
                &live.args(),
                saved_sid,
            ) {
                return Some(live);
            }
        }
    }
    None
}

/// Issue #9 — wait until a freshly woken session is READY, then inject the
/// body through the SHARED per-session lock (D7).
///
/// Slice 5: readiness is now PER-PROVIDER via
/// [`k2_core::workspace::provider_resume::InjectionProfile`]:
/// - `ready_via_bracketed_paste == true` (claude/grok/cursor + the
///   unknown-provider default — claude's shipping behavior, unchanged):
///   poll `bracketed_paste_active()` after the settle floor, up to
///   `wake_timeout`, then inject best-effort. For claude the ?2004h
///   flip genuinely follows input-box mount (original study) — this
///   path is byte-identical to pre-slice-5.
/// - `ready_via_bracketed_paste == false` (codex/gemini/pi/hermes):
///   ?2004h LIES for these providers (set before real readiness, or
///   toggled per repaint), so readiness is the profile's conservative
///   settle floor alone — the WAKE_MIN_SETTLE pattern with a
///   study-derived, per-provider duration (hermes ~7s first-message;
///   codex/gemini 2s; pi 1.5s).
///
/// Returns the [`InjectOutcome`] so the caller maps it to the
/// canonical response.
fn deliver_post_wake(
    live: &session_lookup::LiveSession,
    payload: &str,
    wake_timeout: Duration,
    profile: &k2_core::workspace::provider_resume::InjectionProfile,
) -> InjectOutcome {
    let start = std::time::Instant::now();
    // Minimum settle even if paste mode flips immediately. Never below
    // the historical 400ms floor.
    std::thread::sleep(profile.post_spawn_settle.max(WAKE_MIN_SETTLE));
    if profile.ready_via_bracketed_paste {
        loop {
            if !live.is_child_alive() {
                return InjectOutcome::PtyDied;
            }
            // Ready once the TUI advertises bracketed-paste (claude/grok/
            // cursor set it once the input box is drawn). This is the same
            // signal the injector relies on for framing.
            if live.bracketed_paste_active() {
                break;
            }
            if start.elapsed() >= wake_timeout {
                // Best-effort: timed out waiting for the ready signal — inject
                // anyway (bounded, never blocks forever) rather than dropping
                // the message. Matches the pre-#9 "sleep then send" intent.
                log_debug!(
                    "[msg/wake] session={} readiness wait hit {}ms ceiling — injecting best-effort",
                    live.session_id(),
                    wake_timeout.as_millis()
                );
                break;
            }
            std::thread::sleep(WAKE_POLL_INTERVAL);
        }
    } else if !live.is_child_alive() {
        // Non-polling provider: the settle above WAS the readiness wait.
        return InjectOutcome::PtyDied;
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
    inject_and_submit(live, payload)
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
    from: &str,
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
        let payload = format_message(from, text, command);
        return match inject_and_submit(&live, &payload) {
            InjectOutcome::Delivered => {
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
    let resolved =
        match k2_core::workspace::resume_chat::resolve_resume_chat_args(project_path) {
            Ok(r) => r,
            Err(e) => {
                log_debug!("[msg/wake] resume resolve failed for {project_path}: {e}");
                return MsgResponse::fail(MsgReason::SpawnFailed);
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
    let (spawn_command, spawn_args, wake_timeout, inject_profile) =
        match std::env::var("K2SO_WAKE_HEADLESS_TEST_COMMAND") {
            Ok(c) if !c.is_empty() => (
                c,
                Vec::new(),
                wake_timeout.min(Duration::from_millis(1500)),
                k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE,
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
    }) {
        Ok(o) => o,
        Err(e) => {
            log_debug!("[msg/wake_and_fire] spawn failed: {e}");
            return MsgResponse::fail(MsgReason::SpawnFailed);
        }
    };
    let target_id = outcome.session_id.to_string();

    // Upsert the workspace_sessions row + stamp session_id. Pre-0.37.5
    // history: see git log for `0.37.5 Phase D` if you want the
    // archaeology. The resolver already persisted the identity for the
    // premint/converge cases; the stamp below keeps the resume case's
    // pre-3b parity (idempotent re-write of the same id).
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
        if !resolved.resume_session.is_empty() {
            let _ = WorkspaceSession::update_session_id(
                &conn,
                project_id,
                &resolved.resume_session,
            );
        }
    }

    // The wake IS an activation (#672 invariant: opening/attaching a
    // workspace chat activates it). Bar membership itself derives from
    // live-session PRESENCE (the broadcast unions live sessions in),
    // so the spawn above already re-adds the workspace on the next
    // broadcast — the interaction bump here is for the REAPER's aging
    // window, so it doesn't close the freshly-woken session
    // mid-conversation (pre-fix it armed grace on the next tick), and
    // the suppression clear lifts a pending dismiss (a wake means the
    // workspace is wanted back).
    let _ = k2_core::projects_ops::projects_touch_interaction(project_id);
    crate::active_reaper::clear_dismiss_suppression(project_id);
    crate::active_reaper::recompute_and_broadcast_active();

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
        from
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
    let payload = format_message(from, text, command);
    let _ = Path::new(project_path);
    match deliver_post_wake(&live, &payload, wake_timeout, &inject_profile) {
        InjectOutcome::Delivered => {
            let wake_ms = wake_start.elapsed().as_millis() as u64;
            log_debug!(
                "[daemon/workspace-msg] {} woke session={} in {}ms agent={} from={}",
                branch,
                target_id,
                wake_ms,
                agent_name,
                from
            );
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

    #[test]
    fn format_message_sidecar_from_prefix() {
        assert_eq!(
            format_message("sales/reviewer", "hi", ""),
            "[from sales/reviewer] hi"
        );
        assert_eq!(format_message("sales", "hi", ""), "[from sales] hi");
    }

    #[test]
    fn resolve_msg_target_workspace_handle_and_not_hyphen() {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        let pid = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/msg-target-sales-{pid}");
        conn.execute(
            "INSERT INTO projects (id, name, path) VALUES (?1, 'sales', ?2)",
            rusqlite::params![pid, path],
        )
        .expect("seed sales");
        let sid = uuid::Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO workspace_tab_sessions \
             (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
             VALUES (?1, 'pane-1', 'tab-pane-1', ?2, 'claude', unixepoch())",
            rusqlite::params![pid, sid],
        )
        .expect("tab");
        k2_core::workspace_session_handles::allocate_ordinal(&conn, &pid, &sid)
            .expect("ord");
        drop(conn);

        match resolve_msg_target("sales").expect("canonical") {
            MsgTarget::WorkspaceCanonical { path: p } => assert_eq!(p, path),
            other => panic!("expected canonical, got {other:?}"),
        }
        match resolve_msg_target("sales/1").expect("sidecar 1") {
            MsgTarget::Sidecar {
                handle,
                conversation_key,
                ..
            } => {
                assert_eq!(handle, "1");
                assert_eq!(conversation_key, sid);
            }
            other => panic!("expected sidecar, got {other:?}"),
        }
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO chat_session_names (provider, session_id, custom_name, pinned, updated_at) \
                 VALUES ('claude', ?1, 'Reviewer', 0, unixepoch())",
                rusqlite::params![sid],
            )
            .expect("rename");
        }
        match resolve_msg_target("sales/reviewer").expect("sidecar reviewer") {
            MsgTarget::Sidecar { handle, conversation_key, .. } => {
                assert_eq!(handle, "reviewer");
                assert_eq!(conversation_key, sid);
            }
            other => panic!("expected sidecar reviewer, got {other:?}"),
        }
        assert!(
            resolve_msg_target("sales/1").is_none(),
            "old ordinal must fail after rename"
        );
        assert!(
            resolve_msg_target("sales-reviewer").is_none()
                || matches!(
                    resolve_msg_target("sales-reviewer"),
                    Some(MsgTarget::WorkspaceCanonical { .. })
                ),
            "hyphen must not parse as sidecar"
        );
        if let Some(t) = resolve_msg_target("sales-reviewer") {
            assert!(
                matches!(t, MsgTarget::WorkspaceCanonical { .. }),
                "sales-reviewer is workspace-or-fail, not sidecar: {t:?}"
            );
        }
        assert!(
            resolve_msg_target("agent::host").is_none()
                || matches!(
                    resolve_msg_target("agent::host"),
                    Some(MsgTarget::WorkspaceCanonical { .. })
                )
        );
        assert!(split_abs_stays_workspace());
    }

    fn split_abs_stays_workspace() -> bool {
        k2_core::workspace_session_handles::split_workspace_handle("/Users/foo/sales").is_none()
    }

    #[test]
    fn resolve_msg_target_session_uuid_before_project() {
        k2_core::db::init_for_tests();
        let session_uuid = uuid::Uuid::new_v4().to_string();
        let project_uuid = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, 'uuid-ws', ?2)",
                rusqlite::params![
                    project_uuid,
                    format!("/tmp/msg-uuid-ws-{project_uuid}")
                ],
            )
            .expect("project");
            conn.execute(
                "INSERT INTO workspace_tab_sessions \
                 (project_id, pane_group_id, agent_name, session_id, command, last_seen_at) \
                 VALUES (?1, 'p', 'tab-p', ?2, 'claude', unixepoch())",
                rusqlite::params![project_uuid, session_uuid],
            )
            .expect("tab sid");
        }
        match resolve_msg_target(&session_uuid).expect("session") {
            MsgTarget::Session { session_id } => assert_eq!(session_id, session_uuid),
            other => panic!("session uuid must resolve as session first, got {other:?}"),
        }
        match resolve_msg_target(&project_uuid).expect("project") {
            MsgTarget::WorkspaceCanonical { .. } => {}
            other => panic!("project uuid without session row is workspace, got {other:?}"),
        }
    }

    #[test]
    fn format_message_never_emits_raw_passport_uuid() {
        // Even when callers stamp projects.id as `from`, chat must not
        // show the UUID. Unresolvable ids fall back to "external".
        let uuid = "ee29d27a-4240-4f19-9433-6ed98da6f5a5";
        let out = format_message(uuid, "hello", "");
        assert!(
            !out.contains(uuid),
            "must not put passport uuid in chat prefix: {out}"
        );
        assert!(
            out.starts_with("[from "),
            "must still frame a from prefix: {out}"
        );
    }

    #[test]
    fn format_message_uuid_stamps_handle_not_display() {
        k2_core::db::init_for_tests();
        let id = uuid::Uuid::new_v4().to_string();
        let dir = std::env::temp_dir().join(format!(
            "k2-from-handle-{}-{}",
            std::process::id(),
            &id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().into_owned();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![&id, "Sales Team", &path],
            )
            .unwrap();
            k2_core::workspace::handle::backfill_workspace_handles(&conn);
        }
        let handle = {
            let db = k2_core::db::shared();
            let conn = db.lock();
            k2_core::workspace::handle::project_handle(&conn, &id).expect("handle")
        };
        let out = format_message(&id, "hello", "");
        assert_eq!(out, format!("[from {handle}] hello"));
        assert!(!out.contains("Sales Team"), "display must not stamp [from]");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sidecar_pretty_name_slash_is_rejected() {
        k2_core::db::init_for_tests();
        assert!(
            resolve_msg_target("Sales Team/reviewer").is_none(),
            "pretty name is not a sidecar first segment"
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
    fn pty_stalled_is_transient() {
        // Composer 1a (D2): a stalled lock is transient — a prior send
        // in flight clears, so retrying is sensible (not short-circuited
        // like the permanent reasons).
        assert!(!MsgReason::PtyStalled.is_permanent());
    }

    #[test]
    fn every_reason_has_a_nonempty_hint() {
        for reason in [
            MsgReason::WorkspaceNotFound,
            MsgReason::NoAgentMode,
            MsgReason::SpawnFailed,
            MsgReason::PtyDied,
            MsgReason::PtyStalled,
            MsgReason::DormantNoWake,
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
        assert_eq!(MsgReason::PtyStalled.code(), "pty_stalled");
        assert_eq!(MsgReason::DormantNoWake.code(), "dormant_no_wake");
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
            MsgReason::PtyStalled.code(),
        ] {
            assert_ne!(code, "no_submit", "no_submit must not return");
        }
    }

    // ── Composer 1a (D5) — close-marker sanitization ─────────────────
    //
    // These live alongside the retired-oracle lock-in above per the PRD
    // test plan ("extend the no_submit_reason_is_retired test block").

    #[test]
    fn sanitize_strips_bracketed_paste_close_marker() {
        // A literal ESC[201~ embedded in user text must be neutralized so
        // it cannot end paste mode early and splice live keystrokes
        // (red-team H2 / D5). The ESC is removed; the inert `[201~`
        // remains as literal text inside the paste frame.
        let malicious = "hello\u{1b}[201~rm -rf /";
        let clean = sanitize_inject_text(malicious);
        assert!(
            !clean.contains('\u{1b}'),
            "no raw ESC may survive sanitization: {clean:?}"
        );
        assert_eq!(clean, "hello[201~rm -rf /");
    }

    #[test]
    fn sanitize_strips_all_escape_bytes_including_open_marker() {
        // Both the open (ESC[200~) and close (ESC[201~) markers — and any
        // other escape sequence — are neutralized.
        let s = sanitize_inject_text("\u{1b}[200~payload\u{1b}[201~");
        assert!(!s.contains('\u{1b}'));
        assert_eq!(s, "[200~payload[201~");
    }

    #[test]
    fn sanitize_leaves_clean_text_untouched() {
        assert_eq!(
            sanitize_inject_text("a normal message 123 ✓"),
            "a normal message 123 ✓"
        );
    }

    // ── Composer 1a (D3) — attributed `from`, never empty ────────────

    #[test]
    fn format_message_user_attributes_owner() {
        assert_eq!(format_message_user("owner", "hello"), "[from owner] hello");
    }

    #[test]
    fn format_message_user_is_never_empty_and_never_external() {
        // The route resolves `from` server-side and never passes empty,
        // but defense-in-depth: an empty `from` must still produce a
        // non-empty attribution — and must NOT reuse `k2 msg`'s
        // `external` fallback (red-team C3 / D3).
        let out = format_message_user("", "hi");
        assert!(out.starts_with("[from "), "must carry a prefix: {out}");
        assert!(!out.contains("[from ]"), "from must never be empty: {out}");
        assert!(
            !out.contains("external"),
            "composer must NOT reuse k2 msg's `external` fallback: {out}"
        );
    }

    // ── "Your display name" — owner `from` resolution (D3) ───────────

    #[test]
    fn owner_from_resolves_configured_name_when_set() {
        // A configured name is used verbatim as the attribution.
        assert_eq!(owner_from_or_default(Some("Rosson")), "Rosson");
        assert_eq!(
            format_message_user(&owner_from_or_default(Some("Rosson")), "hi"),
            "[from Rosson] hi"
        );
    }

    #[test]
    fn owner_from_falls_back_to_owner_when_unset_or_blank() {
        // Unset (None), empty, and whitespace-only all fall back to the
        // literal "owner" — the D3 default.
        assert_eq!(owner_from_or_default(None), "owner");
        assert_eq!(owner_from_or_default(Some("")), "owner");
        assert_eq!(owner_from_or_default(Some("   ")), "owner");
        // A name that is ALL control chars also resolves to "owner".
        assert_eq!(owner_from_or_default(Some("\u{1b}\n\t")), "owner");
    }

    #[test]
    fn owner_display_name_strips_esc_and_newlines_before_injection() {
        // The name is embedded in the injected frame, so a raw ESC (paste
        // breakout, red-team H2/D5) or a newline (breaks the one-line
        // prefix) must be neutralized.
        let clean =
            sanitize_owner_display_name("ro\u{1b}sson\nadmin").expect("usable name remains");
        assert!(!clean.contains('\u{1b}'), "no raw ESC may survive: {clean:?}");
        assert!(!clean.contains('\n'), "no newline may survive: {clean:?}");
        assert_eq!(clean, "rossonadmin");

        // End-to-end: a malicious configured name produces an injected
        // frame with NO raw ESC.
        let from = owner_from_or_default(Some("a\u{1b}[201~rm -rf /\nb"));
        let frame = format_message_user(&from, "payload");
        assert!(
            !frame.contains('\u{1b}'),
            "no raw ESC may reach the injected frame: {frame:?}"
        );
        assert!(!frame.contains('\n'), "the `from` segment must stay one-line: {frame:?}");
    }

    #[test]
    fn owner_display_name_is_length_capped() {
        let long = "x".repeat(200);
        let clean = sanitize_owner_display_name(&long).expect("usable");
        assert_eq!(clean.chars().count(), OWNER_DISPLAY_NAME_MAX);
    }

    #[test]
    fn owner_display_name_trims_surrounding_whitespace() {
        assert_eq!(
            sanitize_owner_display_name("  Rosson  ").as_deref(),
            Some("Rosson")
        );
    }

    #[test]
    fn format_message_user_keeps_prefix_on_first_line_only() {
        assert_eq!(
            format_message_user("owner", "line1\nline2"),
            "[from owner] line1\nline2"
        );
    }

    // ── Composer 1a (M1) — live-only `send-message`, no spawn ────────

    #[test]
    fn send_message_unknown_session_returns_pty_died() {
        // A well-formed but unregistered session id resolves to no live
        // PTY → pty_died. Crucially it must NOT spawn anything (M1).
        let r = send_message_to_session(&SessionId::new().to_string(), "owner", "hi");
        assert!(!r.success);
        assert_eq!(r.reason.as_deref(), Some("pty_died"));
        assert!(r.target_session_id.is_none());
    }

    #[test]
    fn send_message_malformed_session_id_returns_pty_died() {
        let r = send_message_to_session("not-a-uuid", "owner", "hi");
        assert!(!r.success);
        assert_eq!(r.reason.as_deref(), Some("pty_died"));
    }

    fn ensure_compose_hist_project(id: &str, path: &str) {
        k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects \
             (id, path, name, color, agent_mode, pinned, tab_order) \
             VALUES (?1, ?2, ?3, '#123456', 'off', 0, 0)",
            rusqlite::params![id, path, "compose-hist"],
        )
        .expect("insert project");
    }

    fn compose_hist_count(project_id: &str) -> i64 {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT COUNT(*) FROM workspace_compose_send_history WHERE project_id = ?1",
            rusqlite::params![project_id],
            |row| row.get(0),
        )
        .expect("count history")
    }

    #[test]
    fn send_message_to_session_does_not_persist_history() {
        // Tickets inject via send_message_to_session. Failed *and*
        // successful injects through that helper must not write history.
        k2_core::db::init_for_tests();
        let pid = format!("csh-ticket-{}", uuid::Uuid::new_v4());
        let path = format!("/tmp/{pid}");
        ensure_compose_hist_project(&pid, &path);
        let before = compose_hist_count(&pid);
        let r = send_message_to_session("not-a-uuid", "owner", "ticket body");
        assert!(!r.success, "inject must fail without a live session");
        assert_eq!(
            compose_hist_count(&pid),
            before,
            "send_message_to_session must never write compose history"
        );
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![pid])
            .expect("cleanup");
    }

    #[test]
    fn persist_compose_send_only_on_success() {
        k2_core::db::init_for_tests();
        let pid = format!("csh-ok-{}", uuid::Uuid::new_v4());
        let path = format!("/tmp/{pid}");
        ensure_compose_hist_project(&pid, &path);

        persist_compose_send_if_delivered(false, &path, "owner", "failed send");
        assert_eq!(
            compose_hist_count(&pid),
            0,
            "failed inject must not store a row"
        );

        persist_compose_send_if_delivered(true, &path, "owner", "   ");
        assert_eq!(
            compose_hist_count(&pid),
            0,
            "empty/whitespace must not store a row"
        );

        persist_compose_send_if_delivered(true, &path, "owner", "delivered line");
        assert_eq!(
            compose_hist_count(&pid),
            1,
            "successful send must persist"
        );

        persist_compose_send_if_delivered(true, "/tmp/k2-compose-hist-unknown-cwd", "owner", "x");
        assert_eq!(
            compose_hist_count(&pid),
            1,
            "unknown cwd must skip insert; send still conceptually succeeds"
        );

        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![pid])
            .expect("cleanup");
    }

    #[test]
    fn compose_history_response_newest_first() {
        k2_core::db::init_for_tests();
        let pid = format!("csh-get-{}", uuid::Uuid::new_v4());
        let path = format!("/tmp/{pid}");
        ensure_compose_hist_project(&pid, &path);
        persist_compose_send_if_delivered(true, &path, "owner", "one");
        persist_compose_send_if_delivered(true, &path, "alice", "two");
        let json = compose_history_response("", &path);
        let v: serde_json::Value = serde_json::from_str(&json).expect("json");
        let bodies: Vec<&str> = v["items"]
            .as_array()
            .expect("items array")
            .iter()
            .map(|i| i["body"].as_str().expect("body"))
            .collect();
        assert_eq!(bodies, vec!["two", "one"], "GET newest-first: {json}");
        let empty = compose_history_response("", "/tmp/k2-compose-hist-no-ws");
        let ev: serde_json::Value = serde_json::from_str(&empty).expect("empty json");
        assert_eq!(
            ev["items"].as_array().expect("items").len(),
            0,
            "unknown path must return empty items"
        );
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![pid])
            .expect("cleanup");
    }

    // ── Composer 1a (D1) — per-session lock keying ───────────────────

    #[test]
    fn inject_lock_same_session_reuses_one_lock_distinct_per_session() {
        // Same session → same Arc, so every injector converges on one
        // lock (D1/D7). Different sessions → distinct Arcs, so one busy
        // PTY never blocks another (D2 isolation, M1 keying).
        let a = SessionId::new();
        let b = SessionId::new();
        let la = lock_for(&a);
        let la2 = lock_for(&a);
        let lb = lock_for(&b);
        assert!(
            Arc::ptr_eq(&la, &la2),
            "same session must reuse exactly one lock"
        );
        assert!(
            !Arc::ptr_eq(&la, &lb),
            "different sessions must get distinct locks"
        );
    }

    // ── Composer 1a (D2/C2) — bounded acquire + isolation ────────────

    #[test]
    fn inject_lock_acquire_is_bounded_and_session_isolated() {
        // D2 half-1 (waiter bound): a waiter for a HELD lock gives up
        // after the deadline (→ pty_stalled) rather than blocking
        // forever. This is the lock-layer model of the wedge case; the
        // real-PTY mapping is exercised in
        // `inject_and_submit_returns_stalled_when_lock_held` below.
        //
        // (Note: a real SIGSTOP child does NOT wedge the *writer* in this
        // architecture — LiveSession::write enqueues onto alacritty's
        // event-loop channel and returns immediately, daemon_pty.rs
        // `write` "Non-blocking" — so the meaningful, testable failure
        // mode is lock contention, exercised directly here.)
        let sid = SessionId::new();
        let held = lock_for(&sid);
        let _g = held.lock();

        let waiter = lock_for(&sid);
        let t0 = std::time::Instant::now();
        let got = waiter.try_lock_for(Duration::from_millis(80));
        assert!(got.is_none(), "waiter must time out while lock is held");
        assert!(
            t0.elapsed() >= Duration::from_millis(60),
            "waiter must have actually waited ~the deadline, waited {:?}",
            t0.elapsed()
        );

        // Isolation: a DIFFERENT session is unaffected by the wedged one.
        let other = lock_for(&SessionId::new());
        assert!(
            other.try_lock_for(Duration::from_millis(80)).is_some(),
            "a healthy session must not be blocked by another session's held lock"
        );
    }

    // ── Composer 1a (anti-splice, LOAD-BEARING) ──────────────────────

    #[test]
    fn inject_lock_serializes_concurrent_injectors_same_session() {
        // The core guarantee: two simultaneous sends + a k2 msg at ONE
        // session never interleave. The anti-splice property IS the lock
        // (not the framing), so we prove it at the lock: three injectors
        // each push Start(id) then End(id) into a shared log while
        // holding the per-session lock. If the lock serializes, every
        // Start is IMMEDIATELY followed by its own End — no interleave.
        // The 15ms hold guarantees that WITHOUT the lock the threads
        // would interleave (they all start near-simultaneously).
        use std::sync::Mutex as StdMutex;

        let sid = SessionId::new();
        let log: Arc<StdMutex<Vec<(char, usize)>>> = Arc::new(StdMutex::new(Vec::new()));

        let mut handles = Vec::new();
        for id in 0..3usize {
            let log = Arc::clone(&log);
            handles.push(std::thread::spawn(move || {
                let lock = lock_for(&sid);
                let _g = lock.lock();
                log.lock().unwrap().push(('S', id));
                std::thread::sleep(Duration::from_millis(15));
                log.lock().unwrap().push(('E', id));
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let events = log.lock().unwrap().clone();
        assert_eq!(events.len(), 6, "3 injectors → 3 Start + 3 End: {events:?}");
        let mut i = 0;
        while i < events.len() {
            let (s, sid_) = events[i];
            assert_eq!(s, 'S', "expected Start at {i}, got {:?}: {events:?}", events[i]);
            let (e, eid) = events[i + 1];
            assert_eq!(
                e, 'E',
                "Start id={sid_} not immediately followed by an End — injectors INTERLEAVED: {events:?}"
            );
            assert_eq!(
                eid, sid_,
                "End id mismatch — injectors INTERLEAVED: {events:?}"
            );
            i += 2;
        }
    }

    // ── Composer 1a — real-PTY integration (Unix only) ───────────────
    //
    // These spawn a real `cat` PTY (cat echoes stdin, never self-exits)
    // so `inject_and_submit` runs against a live child. `cat` does NOT
    // enable bracketed-paste, so injection takes the non-framed write
    // path; the lock + outcome mapping are identical either way.

    #[cfg(unix)]
    fn spawn_cat_live() -> session_lookup::LiveSession {
        use k2_core::terminal::{DaemonPtyConfig, DaemonPtySession, LabelSource};
        use std::path::PathBuf;
        let cfg = DaemonPtyConfig {
            session_id: SessionId::new(),
            cols: 80,
            rows: 24,
            cwd: Some(PathBuf::from("/tmp")),
            program: Some("cat".to_string()),
            args: vec![],
            durable_args: None,
            env: Default::default(),
            drain_on_exit: true,
            label: String::new(),
            label_source: LabelSource::Pty,
            sandbox: k2_core::terminal::SandboxSpec::Passthrough,
            cell_uid: None,
            overlay: None,
        };
        let s = DaemonPtySession::spawn(cfg).expect("spawn cat");
        session_lookup::LiveSession(s)
    }

    #[cfg(unix)]
    #[test]
    fn inject_and_submit_returns_stalled_when_lock_held() {
        // D2 (C2) end-to-end: with the per-session lock already held
        // (simulating an in-flight injection), inject_and_submit returns
        // Stalled within the deadline and does NOT block forever — it
        // never acquired the lock, so it holds nothing. A subsequent
        // send to a HEALTHY session is unaffected (Delivered).
        let live = spawn_cat_live();
        let sid = live.session_id();

        let held = lock_for(&sid);
        let guard = held.lock();

        let live2 = live.clone();
        let t0 = std::time::Instant::now();
        let out = std::thread::spawn(move || {
            inject_and_submit_with_timeout(&live2, "[from owner] hi", Duration::from_millis(150))
        })
        .join()
        .unwrap();
        assert_eq!(
            out,
            InjectOutcome::Stalled,
            "a held lock past the deadline must yield Stalled (→ pty_stalled)"
        );
        assert!(
            t0.elapsed() < Duration::from_secs(2),
            "stalled send must return promptly at the deadline, took {:?}",
            t0.elapsed()
        );
        drop(guard);

        // A healthy, uncontended session delivers normally.
        let healthy = spawn_cat_live();
        let out2 = inject_and_submit_with_timeout(
            &healthy,
            "[from owner] hi",
            Duration::from_secs(2),
        );
        assert_eq!(
            out2,
            InjectOutcome::Delivered,
            "a healthy session must be unaffected by another's wedged lock"
        );

        live.0.kill();
        healthy.0.kill();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_injectors_all_deliver_through_shared_lock() {
        // Two simultaneous composer sends + a k2-msg-style send at ONE
        // live session: all three funnel through the same locked
        // inject_and_submit, serialize, and EACH delivers (none lost,
        // none spliced). This grounds the lock-layer anti-splice proof in
        // the real injection path.
        let live = spawn_cat_live();

        let mut handles = Vec::new();
        for i in 0..3 {
            let l = live.clone();
            handles.push(std::thread::spawn(move || {
                inject_and_submit_with_timeout(
                    &l,
                    &format!("[from owner] msg{i}"),
                    Duration::from_secs(10),
                )
            }));
        }
        let outs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(outs.len(), 3);
        for o in &outs {
            assert_eq!(
                *o,
                InjectOutcome::Delivered,
                "every serialized injector must deliver (none lost): {outs:?}"
            );
        }

        live.0.kill();
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
            hint.contains("k2 work send"),
            "no_agent_mode hint should point to `k2 work send`: {hint}"
        );
        assert!(
            !hint.contains("k2so"),
            "no_agent_mode hint must use the `k2` CLI, not deprecated `k2so`: {hint}"
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
        let r = deliver_live(
            "definitely-not-a-real-workspace-name",
            "hi",
            "sender",
            "",
            false,
            DEFAULT_WAKE_TIMEOUT,
        );
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
        let r = deliver_live("", "hi", "sender", "", false, DEFAULT_WAKE_TIMEOUT);
        assert_eq!(r.reason.as_deref(), Some("workspace_not_found"));
        assert_eq!(r.attempts, 1);
    }

    /// CLI /v1 slug parity: folder basename resolves when display name differs.
    #[test]
    fn resolve_workspace_matches_folder_basename_when_name_differs() {
        k2_core::db::init_for_tests();
        use std::fs;
        let uniq = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
        let base = std::env::temp_dir().join(format!("k2-slug-base-{uniq}"));
        let folder = base.join("sales-interview");
        fs::create_dir_all(&folder).unwrap();
        let project_path = folder.to_string_lossy().into_owned();
        let project_id = uuid::Uuid::new_v4().to_string();
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, agent_enabled, agent_mode) \
                 VALUES (?1, ?2, ?3, 1, 'custom')",
                rusqlite::params![
                    project_id,
                    format!("Sales Interview {uniq}"), // display name ≠ folder
                    project_path,
                ],
            )
            .unwrap();
        }
        let got = resolve_workspace("sales-interview").expect("basename must resolve");
        assert_eq!(got, project_path);
        // Case-insensitive basename
        let got2 = resolve_workspace("Sales-Interview").expect("basename NOCASE");
        assert_eq!(got2, project_path);
    }

    // ── Issue #9 — wake gating + three-state disambiguation ──────────

    #[test]
    fn classify_dormant_distinguishes_the_three_states() {
        // The core ask of #9: the cascade must tell these apart instead of
        // conflating them into one opaque error. #9 RE-FIX: existence is a
        // DB-sourced bool (saved session OR agent_enabled), NOT the persona
        // file — so the classifier now takes `agent_exists: bool`.
        // No agent configured → un-wakeable, regardless of the wake flag.
        assert_eq!(classify_dormant(false, true), DormantDecision::NoAgent);
        assert_eq!(classify_dormant(false, false), DormantDecision::NoAgent);
        // Agent exists → the wake flag decides.
        assert_eq!(classify_dormant(true, false), DormantDecision::NoWake);
        assert_eq!(classify_dormant(true, true), DormantDecision::Wake);
    }

    #[test]
    fn dormant_no_wake_is_permanent_and_has_actionable_hint() {
        assert!(
            MsgReason::DormantNoWake.is_permanent(),
            "dormant_no_wake won't resolve on retry — must short-circuit"
        );
        assert_eq!(MsgReason::DormantNoWake.code(), "dormant_no_wake");
        let hint = MsgReason::DormantNoWake.hint();
        assert!(
            hint.contains("--wake") || hint.contains("talk"),
            "dormant hint must point at the wake remedy: {hint}"
        );
    }

    /// Build a registered, DORMANT workspace in the shared in-memory test
    /// DB and dial the THREE independent existence inputs the #9 re-fix
    /// cares about, so each test can isolate one:
    ///   - `with_persona`: whether a resolvable `.k2/agent/AGENT.md` persona
    ///     FILE exists (the signal the BUGGED prior fix gated on).
    ///   - `with_saved_session`: whether a canonical `workspace_sessions
    ///     .session_id` is saved (dormant = saved, no live terminal).
    ///   - `agent_enabled`: the `projects.agent_enabled` flag.
    /// Returns `(project_id, project_path)`.
    fn scratch_dormant_workspace_ex(
        with_persona: bool,
        with_saved_session: bool,
        agent_enabled: i64,
    ) -> (String, String) {
        use std::fs;
        let uniq = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
        let base = std::env::temp_dir().join(format!("k2-issue9-{uniq}"));
        fs::create_dir_all(&base).unwrap();
        let project_path = base.to_string_lossy().into_owned();
        if with_persona {
            let agent_dir = base.join(".k2").join("agent");
            fs::create_dir_all(&agent_dir).unwrap();
            fs::write(
                agent_dir.join("AGENT.md"),
                "---\nname: scratch-agent\ntype: custom\n---\n# Scratch\n",
            )
            .unwrap();
        }
        let project_id = uuid::Uuid::new_v4().to_string();
        // Keep agent_mode consistent with the enabled flag, matching how
        // production keeps the two in lockstep (schema `Project::update`).
        let agent_mode = if agent_enabled == 1 { "custom" } else { "off" };
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, agent_enabled, agent_mode) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    project_id,
                    format!("scratch-{uniq}"),
                    project_path,
                    agent_enabled,
                    agent_mode
                ],
            )
            .unwrap();
            if with_saved_session {
                let saved_session = uuid::Uuid::new_v4().to_string();
                WorkspaceSession::upsert(
                    &conn,
                    &uuid::Uuid::new_v4().to_string(),
                    &project_id,
                    None,
                    Some(&saved_session),
                    "claude",
                    "system",
                    "sleeping",
                )
                .unwrap();
            }
        }
        (project_id, project_path)
    }

    #[test]
    fn dormant_saved_session_missing_persona_is_wakeable_not_no_agent() {
        // ── #9 RE-FIX, LOAD-BEARING ──────────────────────────────────────
        // The exact testing-canonical shape: a SAVED canonical session_id, a
        // dormant workspace (no live terminal), and NO `.k2/agent/AGENT.md`
        // persona file. Even `agent_enabled=0` here, to prove the SAVED
        // SESSION ALONE establishes existence.
        //
        // Existence MUST be read from the DB, not the persona file:
        //   - The BUGGED prior fix called `find_primary_agent` (reads the
        //     missing file) → None → `no_agent_mode` (the bug).
        //   - The fix: saved_session.is_some() → agent EXISTS → wakeable, so
        //     `msg` WITHOUT --wake yields the distinct `dormant_no_wake`.
        // Asserting `dormant_no_wake` (NOT `no_agent_mode`) IS the proof the
        // existence check consulted the DB and not the file. Using wake=FALSE
        // keeps this assertion spawn-free (no real `claude` launched); the
        // WAKE leg of the same existence is covered by
        // `classify_dormant_distinguishes_the_three_states` (true → Wake).
        let (_pid, path) = scratch_dormant_workspace_ex(false, true, 0);
        let r = attempt_delivery(&path, "hi", "tester", "", false, DEFAULT_WAKE_TIMEOUT);
        assert!(!r.success);
        assert_eq!(
            r.reason.as_deref(),
            Some("dormant_no_wake"),
            "saved session + missing persona must be recognized as a WAKEABLE \
             agent via the DB (dormant_no_wake), NOT no_agent_mode; got {:?}",
            r.reason
        );
        assert_ne!(
            r.reason.as_deref(),
            Some("no_agent_mode"),
            "the #9 bug: existence must NOT be gated on the persona file"
        );
        assert!(r.target_session_id.is_none(), "must not spawn without --wake");
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn agent_enabled_drives_existence_without_saved_session_or_persona() {
        // The OTHER authoritative DB signal: `projects.agent_enabled=1`
        // alone establishes existence even with NO saved session and NO
        // persona file (ProposalWriter/testing-canonical are both
        // agent_enabled=1 in the live DB). wake=FALSE → dormant_no_wake
        // (spawn-free), proving the agent_enabled column — not the file —
        // drives existence.
        let (_pid, path) = scratch_dormant_workspace_ex(false, false, 1);
        let r = attempt_delivery(&path, "hi", "tester", "", false, DEFAULT_WAKE_TIMEOUT);
        assert!(!r.success);
        assert_eq!(
            r.reason.as_deref(),
            Some("dormant_no_wake"),
            "agent_enabled=1 must be recognized as a wakeable agent; got {:?}",
            r.reason
        );
        assert!(r.target_session_id.is_none());
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn no_agent_enabled_and_no_saved_session_returns_no_agent_mode() {
        // The genuinely-no-agent case: agent_enabled=0, NO saved session, NO
        // persona file → un-wakeable. Must surface no_agent_mode (NOT
        // dormant_no_wake, NOT a silent spawn) so the caller gets the "set up
        // an agent" remedy — for BOTH wake values. Also locks the corrected
        // `k2` (not deprecated `k2so`) hint.
        let (_pid, path) = scratch_dormant_workspace_ex(false, false, 0);
        for wake in [false, true] {
            let r = attempt_delivery(&path, "hi", "tester", "", wake, DEFAULT_WAKE_TIMEOUT);
            assert!(!r.success, "no-agent must fail (wake={wake})");
            assert_eq!(
                r.reason.as_deref(),
                Some("no_agent_mode"),
                "no agent_enabled + no saved session must be no_agent_mode (wake={wake})"
            );
            assert!(r.target_session_id.is_none());
            let hint = r.hint.as_deref().unwrap_or("");
            assert!(
                hint.contains("k2 mode custom") && !hint.contains("k2so"),
                "no_agent_mode hint must use the corrected `k2` CLI: {hint}"
            );
        }
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn dormant_with_agent_no_wake_returns_dormant_no_wake_without_spawning() {
        // Has a resolvable agent (persona present + agent_enabled=1) + a
        // saved (sleeping) canonical session; `msg` WITHOUT --wake → the
        // distinct dormant_no_wake. Crucially it must NOT spawn (the "keep
        // msg a dumb blind send" default, #9): a spawn would have returned
        // ok / spawn_failed, never this reason.
        let (_pid, path) = scratch_dormant_workspace_ex(true, true, 1);
        let r = attempt_delivery(&path, "hi", "tester", "", false, DEFAULT_WAKE_TIMEOUT);
        assert!(!r.success);
        assert_eq!(
            r.reason.as_deref(),
            Some("dormant_no_wake"),
            "has-agent dormant + no wake must be dormant_no_wake, got {:?}",
            r.reason
        );
        assert!(r.target_session_id.is_none());
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn resolve_spawn_name_falls_back_to_basename_when_persona_missing() {
        // #9 re-fix: the spawn name must NOT bail to no_agent_mode when the
        // persona file is missing. With no `.k2/agent/AGENT.md`, the name
        // falls back to the workspace folder basename (the product-default
        // display name); with a persona present, its declared `name:` wins.
        use std::fs;
        let uniq = format!("{}-{}", std::process::id(), uuid::Uuid::new_v4());
        let base = std::env::temp_dir().join(format!("k2-issue9-name-{uniq}"));
        fs::create_dir_all(&base).unwrap();
        let path = base.to_string_lossy().into_owned();
        let expected_basename = base.file_name().unwrap().to_string_lossy().into_owned();

        // No persona → basename fallback (NOT empty, NOT a bail).
        let fallback = resolve_spawn_name(&path, "custom");
        assert_eq!(
            fallback, expected_basename,
            "missing persona must fall back to the workspace basename"
        );
        assert!(!fallback.trim().is_empty());

        // Persona present → its declared name wins over the basename.
        let agent_dir = base.join(".k2").join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(
            agent_dir.join("AGENT.md"),
            "---\nname: cortana\ntype: custom\n---\n# Persona\n",
        )
        .unwrap();
        assert_eq!(
            resolve_spawn_name(&path, "custom"),
            "cortana",
            "a present persona's declared name must win over the basename"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    // NOTE on the wake=true positive path: a has-agent dormant workspace
    // with wake=true routes to the real `spawn_agent_session_v2_blocking`
    // (`claude`). We deliberately do NOT drive that end-to-end in a unit
    // test (it would launch a real claude process on dev boxes where it's
    // installed). The wake DECISION is proven by
    // `classify_dormant_distinguishes_the_three_states` (→ Wake) and the
    // post-wake DELIVERY + shared-lock guarantee by
    // `deliver_post_wake_injects_through_the_shared_lock` below — together
    // they cover D7's "wakes + delivers via the shared lock" intent
    // without a live claude.

    #[cfg(unix)]
    #[test]
    fn deliver_post_wake_injects_through_the_shared_lock() {
        // D7 (load-bearing): the post-wake delivery MUST funnel through the
        // SAME per-session INJECT_LOCKS as live sends, so a just-woken
        // session serializes against a concurrent composer (no splice).
        //
        // Proof: hold the session's shared inject lock, then run
        // deliver_post_wake in a thread. If it takes the shared lock it
        // BLOCKS until we release; a private/un-locked path would deliver
        // immediately. `cat` advertises no bracketed-paste, so the
        // readiness wait runs to the (short) timeout, then the inject
        // blocks on the held lock.
        let live = spawn_cat_live();
        let sid = live.session_id();
        let lock = lock_for(&sid);
        let guard = lock.lock(); // hold the SHARED inject lock

        let live2 = live.clone();
        let handle = std::thread::spawn(move || {
            deliver_post_wake(
                &live2,
                "[from owner] hi",
                Duration::from_millis(150),
                &k2_core::workspace::provider_resume::DEFAULT_INJECTION_PROFILE,
            )
        });

        // Past the readiness settle (WAKE_MIN_SETTLE=400ms) it has reached
        // the inject and is blocked on the held shared lock.
        std::thread::sleep(Duration::from_millis(900));
        assert!(
            !handle.is_finished(),
            "deliver_post_wake must block on the SHARED inject lock (proves it acquired it)"
        );

        drop(guard); // release → the injector proceeds
        let out = handle.join().unwrap();
        assert_eq!(
            out,
            InjectOutcome::Delivered,
            "once the shared lock frees, the post-wake body delivers"
        );
        live.0.kill();
    }

    // ── Slice 5 — per-provider readiness selection ───────────────────

    #[cfg(unix)]
    #[test]
    fn deliver_post_wake_non_polling_profile_skips_the_paste_poll() {
        // `cat` NEVER advertises bracketed paste. With a polling
        // (claude-shaped) profile the readiness wait runs to the
        // wake_timeout ceiling; with a non-polling profile (codex/
        // gemini/pi/hermes shape) readiness is the settle floor alone —
        // so with a LONG wake_timeout the send must still return fast.
        // This pins the profile-driven strategy selection.
        use k2_core::workspace::provider_resume::InjectionProfile;
        let live = spawn_cat_live();
        let profile = InjectionProfile {
            ready_via_bracketed_paste: false,
            post_spawn_settle: Duration::from_millis(100),
        };
        let t0 = std::time::Instant::now();
        let out = deliver_post_wake(
            &live,
            "[from owner] hi",
            Duration::from_secs(30), // poll ceiling — must NOT be waited on
            &profile,
        );
        assert_eq!(out, InjectOutcome::Delivered);
        assert!(
            t0.elapsed() < Duration::from_secs(5),
            "non-polling profile must inject after the settle floor, not the 30s poll ceiling (took {:?})",
            t0.elapsed()
        );
        live.0.kill();
    }

    // ── Slice 5 — grok permission-gate hold (hard-safety rule a) ─────

    #[test]
    fn grok_gate_screen_classifier_fires_on_the_verbatim_gate() {
        // Verbatim from tui-signal-study-grok-hermes-cursor.md.
        let rows: Vec<String> = [
            "Run rm -rf build?",
            "",
            "1 (●) Yes, and don't ask again for anything (always-approve mode)",
            "2 (○) Yes, proceed",
            "3 (○) No, reject (type to add feedback)",
            "",
            "1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(screen_shows_grok_gate(&rows), "the open gate must be detected");
    }

    #[test]
    fn grok_gate_screen_classifier_ignores_benign_grok_screens() {
        let busy: Vec<String> = [
            "◆ Thinking…",
            "⠙ Waiting for response… 0.0s   [stop]",
            "Esc:cancel │ Ctrl+.:shortcuts",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert!(!screen_shows_grok_gate(&busy), "a busy grok screen is not a gate");

        let idle: Vec<String> =
            ["hello!", "❯", ""].iter().map(|s| s.to_string()).collect();
        assert!(!screen_shows_grok_gate(&idle), "an idle composer is not a gate");
    }

    #[test]
    fn grok_gate_markers_outside_the_bottom_window_do_not_hold() {
        // A gate string buried in OLD transcript rows (above the bottom
        // scan window) must not hold delivery — only a LIVE gate at the
        // bottom of the screen counts.
        let mut rows: Vec<String> = vec![
            "earlier: 1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel".to_string(),
        ];
        rows.extend((0..GROK_GATE_SCAN_ROWS).map(|i| format!("output line {i}")));
        assert!(
            !screen_shows_grok_gate(&rows),
            "stale gate text above the scan window must not hold"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_grok_pane_displaying_gate_text_is_not_held() {
        // Provider gating: the hold applies ONLY to grok panes. A pane
        // whose child is NOT grok (here: `cat`, which happily echoes the
        // gate text onto its screen) must deliver normally even with the
        // gate strings visible — a claude transcript QUOTING the gate
        // can never block sends.
        let live = spawn_cat_live();
        live.write(b"1/3:select | Ctrl+o:yolo | Ctrl+c:cancel\n")
            .expect("prime the screen with gate-looking text");
        std::thread::sleep(Duration::from_millis(300));
        let out = inject_and_submit_with_timeout(
            &live,
            "[from owner] hi",
            Duration::from_secs(2),
        );
        assert_eq!(
            out,
            InjectOutcome::Delivered,
            "a non-grok pane must never be gate-held"
        );
        live.0.kill();
    }

    #[test]
    fn hitl_gate_open_reason_serializes_with_actionable_hint() {
        let r = MsgResponse::fail(MsgReason::HitlGateOpen);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["success"], false);
        assert_eq!(json["reason"], "hitl_gate_open");
        let hint = json["hint"].as_str().expect("hint present");
        assert!(
            hint.contains("permission gate"),
            "hint must explain the hold: {hint}"
        );
        // Transient (retry-eligible), like pty_stalled — NOT permanent.
        assert!(!MsgReason::HitlGateOpen.is_permanent());
    }

    // ── Slice 5 — rule (c): payload can never escape the paste frame ─

    #[test]
    fn framed_injection_payload_cannot_escape_the_paste_frame() {
        // The framed body is `ESC[200~ <sanitized> ESC[201~`. Because
        // the sanitizer strips EVERY raw ESC from the payload, no
        // payload byte can open/close an escape sequence — every lone
        // `j` / digit / hotkey-shaped char stays INSIDE the paste frame
        // and arrives as literal text (the gemini study showed a bare
        // `j` moving a trust radio; this framing is the guard).
        let hostile = "j 1 2 3 \x1b[201~\rj\x1b[200~ 9";
        let clean = sanitize_inject_text(hostile);
        assert!(
            !clean.contains('\u{1b}'),
            "sanitizer must strip every raw ESC"
        );
        // Reconstruct the exact framed body inject_framed_locked builds.
        let mut body = Vec::with_capacity(clean.len() + 12);
        body.extend_from_slice(b"\x1b[200~");
        body.extend_from_slice(clean.as_bytes());
        body.extend_from_slice(b"\x1b[201~");
        // The ONLY ESC bytes in the frame are the two markers we added:
        // the payload region carries none, so nothing inside can close
        // the paste early.
        let esc_positions: Vec<usize> = body
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == 0x1b)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            esc_positions,
            vec![0, body.len() - 6],
            "exactly the two frame markers may contain ESC"
        );
    }

    #[test]
    fn ok_woke_response_carries_wake_flag_and_duration() {
        // The wake-happened signal the CLI reads to print
        // "peer was dormant — woke (Xs) — delivering".
        let r = MsgResponse::ok_woke("sid-123".into(), "resume_and_fire", 3200);
        assert!(r.success);
        assert!(r.woke);
        assert_eq!(r.wake_ms, Some(3200));
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["woke"], true);
        assert_eq!(json["wake_ms"], 3200);
    }

    #[test]
    fn non_wake_response_omits_wake_fields_from_the_wire() {
        // Lean canonical shape preserved for the common (no-wake) case:
        // `woke`/`wake_ms` are skipped unless a wake actually happened.
        let ok = MsgResponse::ok("sid".into(), "active_terminal_id");
        let json = serde_json::to_value(&ok).unwrap();
        assert!(json.get("woke").is_none(), "woke must be omitted when false");
        assert!(json.get("wake_ms").is_none(), "wake_ms must be omitted when None");

        let fail = MsgResponse::fail(MsgReason::DormantNoWake);
        let fj = serde_json::to_value(&fail).unwrap();
        assert!(fj.get("woke").is_none());
        assert_eq!(fj["reason"], "dormant_no_wake");
    }
}
