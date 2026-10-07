//! Helpers + route handlers shared between the Tauri app's agent-
//! lifecycle HTTP server (`src-tauri/src/agent_hooks.rs`) and the
//! k2so-daemon counterpart (`crates/k2so-daemon/src/main.rs`).
//!
//! Scope:
//! - Event-name canonicalization (`map_event_type`) + URL / query
//!   parsing.
//! - 50-slot recent-event ring buffer used by `k2so hooks status`.
//! - `AgentHookEventSink` trait + `set_sink`/`emit` ambient-singleton
//!   plumbing so both hosts' handlers emit through the same API.
//! - `emit_lifecycle` — the legacy `agent:lifecycle` emit, now fed only
//!   by the daemon's activity store (prd-daemon-activity-and-thread-
//!   working-v1 DA13); no raw hook writes a status any more.
//!
//! The daemon's `AgentHookEventSink` impl publishes events onto
//! its `/events` WebSocket (see `crates/k2so-daemon/src/events.rs`);
//! src-tauri's routes back onto `AppHandle::emit` directly.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

// prd-daemon-activity-and-thread-working-v1 S1: the daemon installs the
// hooks itself (`install`), parses the full hook payload into a typed
// envelope (`envelope`, redacted tool line in `tool_line`), and runs the
// owner check (`owner`) before anything may change a session's activity.
pub mod envelope;
pub mod install;
pub mod owner;
pub mod tool_line;

// ── Host event sink ─────────────────────────────────────────────────────
//
// Agent hooks fire 7 distinct host-facing events; enumerated here so a
// future migration can swap `app_handle.emit(...)` call sites in
// src-tauri/agent_hooks.rs for `sink.emit(HookEvent::...)` without any
// string-match typos. Matches the companion::event_sink shape: set-once
// ambient global with a silent no-op default for daemon / test contexts.

/// Canonical set of events src-tauri/agent_hooks.rs emits to the React
/// frontend. The variant names are kebab-cased in the wire format to
/// match the existing string keys the renderer already listens for.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    AgentLifecycle,
    AgentReply,
    SyncProjects,
    SyncSettings,
    CliTerminalSpawn,
    CliTerminalSpawnBackground,
    CliAiCommit,
    /// Fires when an existing PTY is being surfaced into a tab — i.e.,
    /// the renderer should attach to the given `terminalId` rather
    /// than spawn a new PTY. Differs from `CliTerminalSpawnBackground`
    /// in semantic: Spawn means "I just spawned a new PTY"; Surfaced
    /// means "this PTY is already running, mount a tab on it." See
    /// `.k2so/prds/heartbeat-active-session-tracking.md`.
    SessionSurfaced,
    /// Fires when a workspace session's `surfaced` flag flips 1 → 0
    /// (close-as-minimize). The PTY stays alive in the daemon; every
    /// viewer drops the corresponding tab from its UI so the surface
    /// state stays in sync across windows. Symmetric counterpart to
    /// `SessionSurfaced`. 0.38.0 commit 6.
    SessionUnsurfaced,
    /// Fires when the pinned-chat refresh button kills the workspace's
    /// chat PTY. The originating window remounts its TerminalPane via
    /// `refreshNonce++`; every OTHER viewer needs the same remount so
    /// they don't keep WS handles open to the now-dead session_id.
    /// Payload carries `projectPath` so each window can filter to its
    /// active workspace. 0.38.0 commit 7.
    ChatRefreshed,
    /// Fires when chat-session metadata changes daemon-side (rename,
    /// pin/unpin). The renderer's chat-history sidebar re-fetches its
    /// list. Phase 2 Unit 6 — previously emitted from Tauri's
    /// `chat_history_rename_session` / `chat_history_toggle_pin`
    /// commands; those are now thin proxies (deleted) over the daemon's
    /// `/cli/chat/rename` + `/cli/chat/toggle-pin` routes.
    SyncChatHistory,
    /// Feedback F1 (prd-agent-feedback-notifications §4.3): fires when
    /// an agent files a new feedback item (`/cli/feedback/create`).
    /// Payload: `{id, projectPath, title, kind, priority, agentName}`.
    /// The renderer's Feedback page + waiting-count badge (F2) and the
    /// desktop notification subscribe to this on the `/events` WS.
    FeedbackCreated,
    /// Feedback F1: fires when a feedback item is answered
    /// (`/cli/feedback/answer`). Payload: `{id, projectPath}`.
    /// `k2 feedback ask --wait` polls rather than subscribing in F1,
    /// but the event lets fronts flip a card waiting→answered live.
    FeedbackAnswered,
    /// Feedback: fires when an item's status changes WITHOUT an answer
    /// being recorded (`/cli/feedback/resolve` — resolved / dismissed /
    /// reopened-to-waiting). Payload: `{id, projectPath, status}`.
    /// Lets every window's Feedback list + waiting-count badge refresh
    /// live (answered has its own `FeedbackAnswered` event).
    FeedbackStatusChanged,
    /// Feedback: fires whenever a comment is stored on an item's thread
    /// — `/cli/feedback/comment` (agent- AND human-authored) plus
    /// `/cli/feedback/answer` (a recorded answer also creates a thread
    /// entry). Payload: `{id, projectPath, author}`. INTERNAL refresh
    /// signal only: the renderer refetches the open thread / bumps list
    /// comment counts; it must NEVER trigger the desktop notification
    /// (frozen contract: only NEW items notify, via `FeedbackCreated`).
    FeedbackCommented,
    /// Projects V1 P2 (prd-projects-v1 §4.2): fires when a message is
    /// stored on a project group's single chat stream
    /// (`/cli/project-group/msg`). Payload: `{groupId, groupName,
    /// messageId, author}`. Drives the Projects-tab badge + the live
    /// chat drawer's coalesced refetch. Emitted BEFORE the best-effort
    /// PoC injection, so an open drawer refreshes without waiting on a
    /// slow wake.
    ProjectGroupMessageCreated,
    /// Projects V1 P2: fires when a group's membership changes
    /// (`add-member` / `remove-member`). Payload: `{groupId}`. Drives
    /// the nav member strip + the project Feedback-tab filter refresh.
    ProjectGroupMembersChanged,
    /// Projects V1 P2: fires when a group's Point of Contact changes —
    /// `set-poc`, or the first member auto-becoming PoC on `add-member`.
    /// Payload: `{groupId, pocWorkspaceId}`. Drives the PoC dropdown +
    /// the injection-target display.
    ProjectGroupPocChanged,
    /// Projects V1 P2: fires when a dashboard layout is saved
    /// (`dashboard/save-layout`, revision++). Payload: `{groupId,
    /// dashboardId, revision}`. Freshness signal for the NEXT
    /// open/switch (apply-on-open) — explicitly NOT a live rearrange.
    ProjectGroupLayoutChanged,
    /// Projects V1 P2: structural change to the group LIST — create /
    /// rename / delete / pin / sort. Payload: `{groupId?}`. Drives nav
    /// list liveness (the structural sibling of `sync:projects`; an
    /// implementation addition beyond the ledger's four, PRD §4.2).
    ProjectGroupsChanged,
    /// URLs & Ports drawer — the K2 Connect tunnel's cached nested-
    /// subdomain routing map CHANGED (fired from
    /// `tunnel::subdomains::store()` only when the freshly-fetched map
    /// differs from the cached one). Payload-free refetch nudge: the
    /// daemon's sink reads the canonical map back from
    /// `tunnel::subdomains::current()` when mirroring onto the
    /// session-events bus, so the broadcast can never drift from what
    /// `GET /cli/tunnel/subdomains` serves.
    TunnelSubdomainsChanged,
    /// K2 Mail S2 (prd-email-server-v1 §6.3): fires when a mail
    /// domain's verification status transitions — pending→Verified,
    /// Verified→broken (the daily re-verify regression, `regressed:
    /// true` — the case that must notify), broken→Verified. Payload:
    /// `{domain, status, regressed}`. The Settings→Email page
    /// refreshes its status chips on every fire.
    MailDomainStatusChanged,
    /// K2 Mail S1 (prd-email-server-v1 §4.1): fires when the Stalwart
    /// sidecar's supervised state TRANSITIONS (running → degraded /
    /// stopped, enable finished/failed, disabled, uninstalled).
    /// Payload: `{state, previous, detail?}`. Drives the Settings→Email
    /// status card + the standard app notification on failures. Only
    /// transitions emit — the health cadence never spams steady-state.
    MailServerStateChanged,
    /// K2 Mail S5 (prd-email-server-v1 §8.4): fires when an agent's
    /// outbound message lands in the owner's Approvals queue
    /// (send/reply under `approval` gating). Payload: `{outboundId,
    /// projectPath, agentName}` — deliberately CONTENT-FREE (no
    /// subject/body/recipients, same rule as the feedback push
    /// contract). Drives the Approvals-tab amber dot + the standard
    /// app notification.
    MailSendApprovalRequested,
    /// K2 Mail S5 (prd-email-server-v1 §8.4): fires when a queued
    /// outbound is DECIDED — approved+submitted, denied (with note),
    /// failed at submission, or auto-denied on the 7-day expiry.
    /// Payload: `{outboundId, status}` (the wire status, e.g.
    /// `submitted`/`rejected`/`failed`). An agent's blocked
    /// `send --wait` learns via its own poll; this event refreshes the
    /// Approvals tab + outbox views.
    MailSendDecided,
}

impl HookEvent {
    /// The wire-format event name the React frontend listens for.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::AgentLifecycle => "agent:lifecycle",
            Self::AgentReply => "agent:reply",
            Self::SyncProjects => "sync:projects",
            Self::SyncSettings => "sync:settings",
            Self::CliTerminalSpawn => "cli:terminal-spawn",
            Self::CliTerminalSpawnBackground => "cli:terminal-spawn-background",
            Self::CliAiCommit => "cli:ai-commit",
            Self::SessionSurfaced => "session:surfaced",
            Self::SessionUnsurfaced => "session:unsurfaced",
            Self::ChatRefreshed => "chat:refreshed",
            Self::SyncChatHistory => "sync:chat-history",
            Self::FeedbackCreated => "feedback:created",
            Self::FeedbackAnswered => "feedback:answered",
            Self::FeedbackStatusChanged => "feedback:status-changed",
            Self::FeedbackCommented => "feedback:commented",
            Self::ProjectGroupMessageCreated => "project-group:message-created",
            Self::ProjectGroupMembersChanged => "project-group:members-changed",
            Self::ProjectGroupPocChanged => "project-group:poc-changed",
            Self::ProjectGroupLayoutChanged => "project-group:layout-changed",
            Self::ProjectGroupsChanged => "project-group:groups-changed",
            Self::TunnelSubdomainsChanged => "tunnel:subdomains-changed",
            Self::MailDomainStatusChanged => "mail:domain-status-changed",
            Self::MailServerStateChanged => "mail:server-state-changed",
            Self::MailSendApprovalRequested => "mail:send-approval-requested",
            Self::MailSendDecided => "mail:send-decided",
        }
    }
}

/// Abstraction for "how do agent-hook emissions reach the UI." The
/// Tauri app provides an impl that calls `AppHandle::emit`; the future
/// k2so-daemon provides one that fans out over the companion WS.
pub trait AgentHookEventSink: Send + Sync {
    fn emit(&self, event: HookEvent, payload: serde_json::Value);
}

static SINK: OnceLock<Mutex<Option<Box<dyn AgentHookEventSink>>>> = OnceLock::new();

fn sink_slot() -> &'static Mutex<Option<Box<dyn AgentHookEventSink>>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Register the host's sink. Idempotent; last writer wins (tests).
pub fn set_sink(s: Box<dyn AgentHookEventSink>) {
    *sink_slot().lock() = Some(s);
}

/// Fire `event` through the registered sink, if any. Silent no-op if
/// unregistered — daemon smoke tests + early-startup paths don't need
/// to care.
pub fn emit(event: HookEvent, payload: serde_json::Value) {
    if let Some(s) = sink_slot().lock().as_ref() {
        s.emit(event, payload);
    }
}

// ── Recent-event ring buffer ────────────────────────────────────────────

const RECENT_EVENTS_CAP: usize = 50;

/// Past-events ring buffer used by `k2so hooks status` and friends.
/// Keyed by insertion order; newest at the back.
static RECENT_EVENTS: OnceLock<Mutex<VecDeque<RecentEvent>>> = OnceLock::new();

/// One ring-buffer row. DA5: event name, tool name, verdict and time only
/// (plus the pane ids), never payload content.
#[derive(Clone, Debug, serde::Serialize)]
pub struct RecentEvent {
    pub timestamp: String,
    pub raw_event: String,
    pub canonical: Option<String>,
    pub pane_id: String,
    pub tab_id: String,
    pub matched: bool,
    /// `POST /hook/event` only: the tool the event is about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// `POST /hook/event` only: the owner-check verdict (`accepted`,
    /// `foreign`, `unknown_pane`, `parse_error`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verdict: Option<String>,
}

fn recent_events() -> &'static Mutex<VecDeque<RecentEvent>> {
    RECENT_EVENTS.get_or_init(|| Mutex::new(VecDeque::with_capacity(RECENT_EVENTS_CAP)))
}

/// Append a new hook event to the ring buffer. Oldest entry evicted
/// when at capacity.
pub fn record_recent_event(
    raw: &str,
    canonical: Option<&str>,
    pane_id: &str,
    tab_id: &str,
) {
    record_recent_hook_event(raw, canonical, pane_id, tab_id, None, None);
}

/// [`record_recent_event`] with the `/hook/event` extras (tool name and
/// owner-check verdict).
pub fn record_recent_hook_event(
    raw: &str,
    canonical: Option<&str>,
    pane_id: &str,
    tab_id: &str,
    tool_name: Option<&str>,
    verdict: Option<&str>,
) {
    let event = RecentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        raw_event: raw.to_string(),
        canonical: canonical.map(String::from),
        pane_id: pane_id.to_string(),
        tab_id: tab_id.to_string(),
        matched: canonical.is_some(),
        tool_name: tool_name.map(String::from),
        verdict: verdict.map(String::from),
    };
    let mut buf = recent_events().lock();
    if buf.len() >= RECENT_EVENTS_CAP {
        buf.pop_front();
    }
    buf.push_back(event);
}

/// Snapshot the recent events (newest last). Exposed for the CLI's
/// `k2so hooks status` probe.
pub fn get_recent_events() -> Vec<RecentEvent> {
    recent_events().lock().iter().cloned().collect()
}

/// Test helper: clear the ring buffer. Available under cfg(test) and
/// when the `test-util` feature is on so downstream crates' test
/// binaries can reset the global state between assertions.
#[cfg(any(test, feature = "test-util"))]
pub fn clear_recent_events() {
    recent_events().lock().clear();
}

/// Canonical lifecycle event name that downstream code should switch on.
/// Surfaced to the frontend via `agent:lifecycle` emits (event_type = one
/// of these strings). Variants are lifted from the CLI hook scripts and
/// third-party agents (Claude, Cursor, Gemini) — any new value goes into
/// one of the three buckets below.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLifecycleEvent {
    pub pane_id: String,
    pub tab_id: String,
    /// One of "start" / "stop" / "permission".
    pub event_type: String,
    /// Absolute workspace / project path this pane belongs to (daemon-
    /// authoritative). Clients map this to a project id for Active-bar
    /// attribution so remote hosts never depend on local tab stashes.
    /// `None` when the pane cannot be resolved (unknown session).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_path: Option<String>,
}

/// Map a raw hook event name onto the three-bucket canonical taxonomy.
/// `None` means the event isn't one we care about.
pub fn map_event_type(raw: &str) -> Option<&'static str> {
    match raw {
        // Start events
        "Start" | "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure"
        | "BeforeAgent" | "AfterTool" | "sessionStart" | "userPromptSubmitted"
        | "postToolUse" | "beforeSubmitPrompt" => Some("start"),

        // Stop events
        "Stop" | "agent-turn-complete" | "AfterAgent" | "sessionEnd" | "stop" => {
            Some("stop")
        }

        // Permission request events
        "PermissionRequest" | "Notification" | "preToolUse"
        | "beforeShellExecution" | "beforeMCPExecution" => Some("permission"),

        _ => None,
    }
}

/// Parse the query-string from a URL like
/// `/hook/complete?paneId=...&tabId=...&eventType=...` into a map.
/// Each value is URL-decoded via [`urldecode`].
pub fn parse_query_params(url: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query) = url.split('?').nth(1) {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                let decoded = urldecode(value);
                params.insert(key.to_string(), decoded);
            }
        }
    }
    params
}

// ── Route handlers ──────────────────────────────────────────────────────
//
// Each `handle_*` function takes an already-parsed params map and runs
// the pure business logic for one route. Token auth happens in the
// calling HTTP layer (daemon or src-tauri) so these stay protocol-
// agnostic. Hosts wrap the return with HTTP serialization.

/// Resolve the workspace path that owns `pane_id` for lifecycle broadcast
/// attribution. Prefer the registered project's path for this terminal;
/// fall back to the hook's PWD (`cwd` query param). Never invents a path.
pub fn resolve_workspace_path_for_pane(
    pane_id: &str,
    hook_cwd: Option<&str>,
) -> Option<String> {
    if !pane_id.is_empty() {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Ok(Some(s)) =
            crate::db::schema::WorkspaceSession::get_by_terminal_id(&conn, pane_id)
        {
            if let Ok(p) = crate::db::schema::Project::get(&conn, &s.project_id) {
                if !p.path.is_empty() {
                    return Some(p.path);
                }
            }
        }
    }
    hook_cwd
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Fire the legacy lifecycle outputs for one canonical bucket
/// (`start` | `stop` | `permission`): the `/events` `agent:lifecycle`
/// frame, which the daemon's sink also mirrors onto the session-events
/// bus as `agent_status_changed`.
///
/// prd-daemon-activity-and-thread-working-v1 DA13 / RL5: the daemon's
/// activity store is the only caller. It derives the bucket from its own
/// row (display), never from a raw hook, and it alone writes
/// `workspace_sessions.status` (DA32). Raw hooks no longer map straight
/// onto a status: `handle_hook_complete`'s status write is gone.
pub fn emit_lifecycle(pane_id: &str, canonical: &str, workspace_path: Option<&str>) {
    let event = AgentLifecycleEvent {
        pane_id: pane_id.to_string(),
        tab_id: pane_id.to_string(),
        event_type: canonical.to_string(),
        workspace_path: workspace_path.map(str::to_string),
    };
    crate::log_debug!(
        "[agent-hooks] lifecycle {} (pane={}, path={:?})",
        canonical,
        pane_id,
        workspace_path
    );
    emit(
        HookEvent::AgentLifecycle,
        serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
    );
}

/// The `/cli/hooks/status` injection report for the daemon user's home:
/// per CLI `{path, exists, injected, events, configUnreadable}` plus the
/// script's path, presence and version stamp. `injected` means at least
/// one event holds a K2 entry; `events` lists which. See
/// [`install::check_hook_injections`] (DA17, DA18).
pub fn check_hook_injections() -> serde_json::Value {
    match dirs::home_dir() {
        Some(home) => install::check_hook_injections(&home),
        None => serde_json::json!({
            "notify_script": { "path": "", "exists": false, "version": null },
            "claude": { "path": null, "exists": false, "injected": false, "events": [], "configUnreadable": false },
            "cursor": { "path": null, "exists": false, "injected": false, "events": [], "configUnreadable": false },
            "gemini": { "path": null, "exists": false, "injected": false, "events": [], "configUnreadable": false },
        }),
    }
}

/// Percent-decode a URL-encoded string. Handles multi-byte UTF-8 sequences
/// (e.g. `%E2%80%94` → `—`) by decoding bytes into a buffer first, then
/// converting to UTF-8.
pub fn urldecode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                } else {
                    bytes.push(b'%');
                    bytes.extend_from_slice(hex.as_bytes());
                }
            } else {
                bytes.push(b'%');
                bytes.extend_from_slice(hex.as_bytes());
            }
        } else if c == '+' {
            bytes.push(b' ');
        } else {
            let mut buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|e| {
        String::from_utf8_lossy(e.into_bytes().as_slice()).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PLMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    #[test]
    fn hook_event_name_matches_wire_format() {
        assert_eq!(HookEvent::AgentLifecycle.event_name(), "agent:lifecycle");
        assert_eq!(HookEvent::AgentReply.event_name(), "agent:reply");
        assert_eq!(HookEvent::SyncProjects.event_name(), "sync:projects");
        assert_eq!(HookEvent::SyncSettings.event_name(), "sync:settings");
        assert_eq!(
            HookEvent::CliTerminalSpawn.event_name(),
            "cli:terminal-spawn"
        );
        assert_eq!(
            HookEvent::CliTerminalSpawnBackground.event_name(),
            "cli:terminal-spawn-background"
        );
        assert_eq!(HookEvent::CliAiCommit.event_name(), "cli:ai-commit");
        // Projects V1 P2 — the five project-group wire names (PRD §4.2).
        assert_eq!(
            HookEvent::ProjectGroupMessageCreated.event_name(),
            "project-group:message-created"
        );
        assert_eq!(
            HookEvent::ProjectGroupMembersChanged.event_name(),
            "project-group:members-changed"
        );
        assert_eq!(
            HookEvent::ProjectGroupPocChanged.event_name(),
            "project-group:poc-changed"
        );
        assert_eq!(
            HookEvent::ProjectGroupLayoutChanged.event_name(),
            "project-group:layout-changed"
        );
        assert_eq!(
            HookEvent::ProjectGroupsChanged.event_name(),
            "project-group:groups-changed"
        );
        // URLs & Ports — the tunnel nested-subdomain map change nudge.
        assert_eq!(
            HookEvent::TunnelSubdomainsChanged.event_name(),
            "tunnel:subdomains-changed"
        );
        // K2 Mail S2 — domain verification transitions (§6.3).
        assert_eq!(
            HookEvent::MailDomainStatusChanged.event_name(),
            "mail:domain-status-changed"
        );
        // K2 Mail S5 — send approvals (§8.4).
        assert_eq!(
            HookEvent::MailSendApprovalRequested.event_name(),
            "mail:send-approval-requested"
        );
        assert_eq!(HookEvent::MailSendDecided.event_name(), "mail:send-decided");
    }

    #[test]
    fn emit_without_sink_is_silent_noop() {
        let _g = TEST_LOCK.lock();
        *sink_slot().lock() = None;
        emit(HookEvent::AgentLifecycle, serde_json::json!({}));
    }

    #[test]
    fn registered_sink_receives_emit() {
        let _g = TEST_LOCK.lock();
        let count = Arc::new(AtomicUsize::new(0));
        struct Fake(Arc<AtomicUsize>);
        impl AgentHookEventSink for Fake {
            fn emit(&self, _e: HookEvent, _p: serde_json::Value) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        set_sink(Box::new(Fake(count.clone())));
        emit(HookEvent::SyncProjects, serde_json::json!({}));
        emit(HookEvent::AgentReply, serde_json::json!({"x": 1}));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        *sink_slot().lock() = None;
    }

    #[test]
    fn map_event_type_buckets() {
        assert_eq!(map_event_type("Start"), Some("start"));
        assert_eq!(map_event_type("UserPromptSubmit"), Some("start"));
        assert_eq!(map_event_type("Stop"), Some("stop"));
        assert_eq!(map_event_type("agent-turn-complete"), Some("stop"));
        assert_eq!(map_event_type("PermissionRequest"), Some("permission"));
        assert_eq!(map_event_type("beforeShellExecution"), Some("permission"));
        assert_eq!(map_event_type("unknown"), None);
    }

    #[test]
    fn parse_query_params_basic() {
        let params = parse_query_params("/hook/complete?paneId=t-1&tabId=x&eventType=Start");
        assert_eq!(params.get("paneId"), Some(&"t-1".to_string()));
        assert_eq!(params.get("tabId"), Some(&"x".to_string()));
        assert_eq!(params.get("eventType"), Some(&"Start".to_string()));
    }

    #[test]
    fn parse_query_params_url_decodes_values() {
        let params = parse_query_params("/hook?message=hello%20world&symbol=%E2%80%94");
        assert_eq!(params.get("message"), Some(&"hello world".to_string()));
        assert_eq!(params.get("symbol"), Some(&"—".to_string()));
    }

    #[test]
    fn parse_query_params_no_query_is_empty() {
        let params = parse_query_params("/hook/status");
        assert!(params.is_empty());
    }

    #[test]
    fn urldecode_handles_multibyte_utf8() {
        // "café" with é = %C3%A9
        assert_eq!(urldecode("caf%C3%A9"), "café");
        // em dash
        assert_eq!(urldecode("a%E2%80%94b"), "a—b");
    }

    #[test]
    fn urldecode_preserves_invalid_percent_sequences() {
        // "%GG" is not a valid hex escape — kept as-is.
        assert_eq!(urldecode("a%GGb"), "a%GGb");
    }

    #[test]
    fn urldecode_plus_becomes_space() {
        assert_eq!(urldecode("hello+world"), "hello world");
    }

    #[test]
    fn recent_events_ring_buffer_fifo_eviction() {
        let _g = TEST_LOCK.lock();
        // Clear between tests.
        recent_events().lock().clear();

        for i in 0..(RECENT_EVENTS_CAP + 5) {
            record_recent_event(&format!("Event{i}"), Some("start"), "pane", "tab");
        }

        let events = get_recent_events();
        assert_eq!(events.len(), RECENT_EVENTS_CAP);
        // Oldest 5 should be evicted; first retained is Event5.
        assert_eq!(events.first().unwrap().raw_event, "Event5");
        // Newest at the back.
        assert_eq!(
            events.last().unwrap().raw_event,
            format!("Event{}", RECENT_EVENTS_CAP + 4)
        );
    }

    #[test]
    fn emit_lifecycle_sends_one_agent_lifecycle_frame() {
        let _g = TEST_LOCK.lock();
        let captured: Arc<PLMutex<Vec<(String, serde_json::Value)>>> =
            Arc::new(PLMutex::new(Vec::new()));
        struct Fake(Arc<PLMutex<Vec<(String, serde_json::Value)>>>);
        impl AgentHookEventSink for Fake {
            fn emit(&self, event: HookEvent, payload: serde_json::Value) {
                self.0.lock().push((event.event_name().to_string(), payload));
            }
        }
        set_sink(Box::new(Fake(captured.clone())));

        emit_lifecycle("pane-9", "start", Some("/home/u/ws"));

        let events = captured.lock();
        assert_eq!(events.len(), 1, "expected one emit, got {events:?}");
        assert_eq!(events[0].0, "agent:lifecycle");
        assert_eq!(events[0].1["eventType"], "start");
        assert_eq!(events[0].1["paneId"], "pane-9");
        assert_eq!(events[0].1["tabId"], "pane-9");
        assert_eq!(events[0].1["workspacePath"], "/home/u/ws");
        drop(events);
        *sink_slot().lock() = None;
    }

    #[test]
    fn recent_event_records_canonical_and_matched() {
        let _g = TEST_LOCK.lock();
        recent_events().lock().clear();
        record_recent_event("Start", Some("start"), "p1", "t1");
        record_recent_event("GarbageEvent", None, "p2", "t2");
        let events = get_recent_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].canonical.as_deref(), Some("start"));
        assert!(events[0].matched);
        assert_eq!(events[1].canonical, None);
        assert!(!events[1].matched);
    }
}
