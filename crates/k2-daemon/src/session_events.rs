//! 0.38.0 Commit 4 — daemon-owned session lifecycle broadcast.
//!
//! Push-driven counterpart to `/cli/sessions/list-for-workspace`.
//! Every `v2_session_map::register` / `unregister` call publishes a
//! `SessionEvent` onto a process-wide `tokio::sync::broadcast` channel.
//! `session_events_ws::serve_session_events_connection` fans those
//! events out to each `/cli/sessions/events?path=<workspace>` WS
//! subscriber, filtering by `cwd starts_with workspace_path` (same
//! rule the existing list-for-workspace endpoint uses in `cli.rs`).
//!
//! Why a broadcast bus instead of per-subscriber polling: the
//! renderer used to discover new daemon-owned PTYs only on workspace
//! switch (`loadLayoutForWorkspace` triggers `reconcileWithDaemon`).
//! That left every other connected window stale until the user
//! manually re-opened the workspace. Push-driven adoption + drop
//! means Cmd+T in window A surfaces the new tab in window B + the
//! mobile companion within one network hop.
//!
//! Initialization: a `OnceLock<Sender<SessionEvent>>` lazily creates
//! the channel on first `sender()` call. Test environments that
//! never call into `register`/`unregister` won't initialize it;
//! emit sites use `let _ = ` to ignore the case where the bus has
//! no live subscribers (broadcast::send returns an Err in that
//! case but we treat it as a no-op).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tokio::sync::broadcast;

/// Capacity for the broadcast channel. Small enough that a stalled
/// subscriber doesn't bloat memory, large enough that bursts (e.g.
/// opening 10 tabs back-to-back) don't drop events under realistic
/// network jitter. Lagged subscribers get `RecvError::Lagged` and
/// the WS handler in `session_events_ws.rs` treats that as a hint
/// to re-emit the current truth via a fresh snapshot.
pub const EVENT_CHANNEL_CAP: usize = 256;

/// One lifecycle event for a daemon-owned PTY session. Serialized
/// as JSON and pushed to every matching WS subscriber. Discriminated
/// by `kind` so callers can switch on the variant cheaply.
///
/// **Wire stability:** the renderer + mobile companion both consume
/// this shape. Field renames are breaking. Adding new variants is
/// safe — clients should ignore unknown `kind` values.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A daemon-owned PTY session has been registered in the v2 map.
    /// Emitted from `v2_session_map::register`. `agent_name` is the
    /// canonical key under which the session was registered (e.g.
    /// `tab-<paneGroupId>` for Cmd+T tabs, bare project UUID for
    /// pinned chat, ad-hoc shapes for heartbeats).
    SessionAdded {
        /// Absolute cwd of the spawned PTY. Subscribers filter on
        /// `cwd starts_with subscriber's workspace_path` before
        /// forwarding to their socket.
        workspace_path: String,
        /// `tab-<paneGroupId>` extracted from `agent_name` when the
        /// prefix matches; absent otherwise (pinned chat, heartbeat).
        /// The renderer uses this as the canonical paneGroup id when
        /// adopting the session as a new tab.
        pane_group_id: Option<String>,
        /// Canonical `v2_session_map` key. The renderer filters on
        /// the `tab-` prefix to decide whether to surface the event
        /// as a tab; non-tab agent_names are forwarded so the mobile
        /// companion sees the full session inventory.
        agent_name: String,
        /// Spawn command. May be `null` when the daemon spawned the
        /// user's default login shell.
        command: Option<String>,
        /// Spawn args. Empty when none.
        args: Vec<String>,
        /// Daemon-side session UUID. Mirrors
        /// `list-for-workspace`'s `sessionId` field.
        session_id: String,
        /// Always `true` for v2 sessions today. Reserved for the
        /// post-cleanup phase that may add legacy events.
        #[serde(rename = "isV2")]
        is_v2: bool,
        /// P3c (D1) — the resolved sandbox backend name, present ONLY when
        /// this session runs under a REAL sandbox (e.g. `"microvm"`); `None`
        /// for the default passthrough / bare-PTY path. This mirrors the
        /// `v2/spawn` echo rule (only-surface-a-real-backend) so a non-sandbox
        /// `SessionAdded` is byte-identical to pre-P3c (the renderer's generic
        /// tab-adoption consumer reads it to light the D9 orange marker
        /// immediately on an externally / API-spawned cell).
        ///
        /// **Additive + serde-optional.** Skipped from the wire when `None`
        /// so older clients ignore it and the default path emits no new key.
        #[serde(skip_serializing_if = "Option::is_none")]
        sandbox_backend: Option<String>,
    },
    /// A session has been removed from the v2 map. Emitted from
    /// `v2_session_map::unregister` (which is the single chokepoint
    /// for "session goes away" — child exit observer + explicit
    /// close route + watchdog escalation all funnel through it).
    SessionRemoved {
        workspace_path: String,
        pane_group_id: Option<String>,
        agent_name: String,
    },
    /// Canonical Active-set changed (task #672 — daemon-owned Active).
    /// Carries the FULL set (not a diff) so client convergence is
    /// trivial + race-free (last-write-wins on a monotonic snapshot).
    /// Emitted after every recompute that can change membership:
    /// activate / pin / dismiss / window-tick / reaper close.
    ///
    /// **App-level, not workspace-scoped.** Unlike `SessionAdded`/
    /// `SessionRemoved` (filtered by `cwd starts_with path`), this
    /// event has no single workspace — `session_events_ws` forwards it
    /// to EVERY subscriber regardless of their `?path=` so each client
    /// mirrors the global union (see `event_matches_workspace`).
    ///
    /// **Wire shape (frozen — the renderer codes against these EXACT
    /// names):** `{ "kind": "active_changed", "activeProjectIds":
    /// string[], "activeWindowHours": number }`. The `#[serde(tag =
    /// "kind", rename_all = "snake_case")]` on the enum yields the
    /// `active_changed` tag; the per-field `rename` yields the camelCase
    /// field names.
    ActiveChanged {
        #[serde(rename = "activeProjectIds")]
        active_project_ids: Vec<String>,
        #[serde(rename = "activeWindowHours")]
        active_window_hours: u32,
    },
    /// Session label/title changed. Reserved variant — currently
    /// unused on the emit side because the daemon's existing label
    /// broadcast (`sessions_grid_ws::Outbound::LabelChanged`) is
    /// already wired into the per-session WS. Kept in the schema
    /// so a future caller (mobile companion: "what's the current
    /// title?") doesn't break wire compat.
    #[allow(dead_code)]
    SessionRenamed {
        workspace_path: String,
        pane_group_id: Option<String>,
        title: String,
    },

    // ─── 0.39.39 daemon-canonical broadcast spine (#675/#676/#677) ───
    //
    // These variants let the renderer DROP its polling intervals and
    // subscribe to the daemon's source-of-truth instead. Each is emitted
    // at the daemon's canonical mutation point. Wire-frozen: the renderer
    // Wave-C consumers code against the EXACT `kind` tag + field names
    // below. Adding fields later is breaking — append a new variant
    // instead. Field names are camelCase on the wire (per-field
    // `rename`), matching the existing SessionEvent convention.
    //
    // **Routing classes** (see `session_events_ws::event_matches_workspace`):
    //   - APP-LEVEL  (forwarded to EVERY subscriber regardless of `?path=`):
    //     `LlmStatusChanged`, `TunnelStatusChanged`, `AgentStatusChanged`,
//     `SessionActivityChanged`.
    //   - WORKSPACE-SCOPED (forwarded only when the carried path matches
    //     the subscriber's `?path=` via the cwd-prefix rule):
    //     `ReviewQueueChanged`, `ReviewChanged`, `TabTitleChanged`,
    //     `TabOrderChanged`, `HeartbeatStateChanged`,
    //     `HeartbeatRosterChanged` — each carries a `workspacePath` used
    //     by the prefix filter.

    /// #675.1 — local LLM (AI Workspace Assistant) model status changed.
    /// APP-LEVEL. Replaces the renderer's `/cli/llm/status` poll
    /// (assistant.ts). Emitted on every transition that the renderer
    /// cares about: model load, download begin/end, and download
    /// progress ticks.
    ///
    /// Wire: `{ "kind": "llm_status_changed", "loaded": bool,
    /// "modelPath": string|null, "downloading": bool,
    /// "downloadPercent": number|null }`.
    ///
    /// `downloadPercent` is `Some(0.0..=100.0)` while a download is in
    /// flight (carried on the progress emits), `None` otherwise.
    LlmStatusChanged {
        loaded: bool,
        #[serde(rename = "modelPath")]
        model_path: Option<String>,
        downloading: bool,
        #[serde(rename = "downloadPercent")]
        download_percent: Option<f64>,
    },

    /// #675.2 — an agent's working/idle status flipped. APP-LEVEL.
    /// Replaces the renderer's terminal/list-running + agent-status poll
    /// (active-agents.ts) so spinners are push-driven. Emitted from the
    /// daemon's agent-lifecycle hook chokepoint
    /// (`handle_hook_complete`), which maps every raw harness hook into
    /// the canonical `start`/`stop`/`permission` buckets.
    ///
    /// Wire: `{ "kind": "agent_status_changed", "paneId": string,
    /// "tabId": string, "status": "start"|"stop"|"permission" }`.
    ///
    /// `paneId` is the `K2SO_PANE_ID` (== terminal id) the PTY was
    /// spawned with — the same key `agent:lifecycle` carries today and
    /// the renderer already correlates spinners against.
    AgentStatusChanged {
        #[serde(rename = "paneId")]
        pane_id: String,
        #[serde(rename = "tabId")]
        tab_id: String,
        /// Canonical bucket: `start` (working) | `stop` (idle) |
        /// `permission` (awaiting approval).
        status: String,
    },

    /// 0.40.39 — daemon-side per-session activity (session_activity.rs,
    /// the 0.40.23 deferred item). Title/Bell-derived working|idle|
    /// permission, emitted on TRANSITIONS only, for EVERY live session
    /// regardless of any client's pane visibility. APP-LEVEL routing.
    /// Distinct from `AgentStatusChanged` (hook-driven lifecycle whose
    /// `stop` arm drives toasts/review): this is high-frequency turn
    /// state, consumed only by the activity store.
    /// Wire: `{ "kind": "session_activity_changed", "workspacePath":
    /// string, "agentName": string, "paneGroupId": string|null,
    /// "status": "working"|"idle"|"permission" }`.
    SessionActivityChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        #[serde(rename = "agentName")]
        agent_name: String,
        #[serde(rename = "paneGroupId")]
        pane_group_id: Option<String>,
        status: String,
    },

    /// #675.3 — the per-workspace review queue changed (a review was
    /// approved / rejected / had changes requested, or a checklist
    /// mutation altered queue membership). WORKSPACE-SCOPED. Replaces
    /// the renderer's `/cli/agents/review-queue` poll (review-queue.ts).
    ///
    /// Wire: `{ "kind": "review_queue_changed", "workspacePath":
    /// string }`. The renderer re-fetches the queue for the matching
    /// workspace on receipt.
    ReviewQueueChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
    },

    /// #675.4 + #677.2 — a specific review (or its checklist) changed.
    /// WORKSPACE-SCOPED. Replaces the renderer's reviews+chats poll
    /// (ReviewPanel.tsx) AND the review-checklist poll — ONE event
    /// covers both. Emitted on review approve/reject/request-changes
    /// and on every review-checklist write/toggle/init.
    ///
    /// Wire: `{ "kind": "review_changed", "workspacePath": string,
    /// "agent": string|null }`. `agent` is the agent/branch the review
    /// belongs to when the mutation point knows it (checklist + queue
    /// mutations carry it); `null` for queue-wide changes. The renderer
    /// re-fetches the review detail + checklist for the matching
    /// workspace on receipt.
    ReviewChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        agent: Option<String>,
    },

    /// #675.5 — K2 Connect tunnel connector state transitioned.
    /// APP-LEVEL. Replaces the renderer's `/cli/tunnel/status` poll
    /// (CompanionSection.tsx). Emitted on start / stop. Carries the
    /// new `running` flag + predicted public URL so the renderer can
    /// render without an immediate follow-up GET.
    ///
    /// Wire: `{ "kind": "tunnel_status_changed", "running": bool,
    /// "publicUrl": string|null }`.
    TunnelStatusChanged {
        running: bool,
        #[serde(rename = "publicUrl")]
        public_url: Option<String>,
    },

    /// URLs drawer / K2 Connect settings — the K2 Connect tunnel's cached
    /// NESTED-subdomain routing map changed (the daemon-owned refresh loop
    /// landed a map that differs from the cached one; see
    /// `k2_core::tunnel::subdomains::store`), OR a label's WORKSPACE
    /// attribution changed (0074 claim/unclaim — same event so both UIs
    /// refresh from one signal). APP-LEVEL — one tunnel per daemon, the
    /// `TunnelStatusChanged` routing class. Carries the WHOLE map (not a
    /// diff) so the renderer converges without a follow-up GET; the
    /// snapshot twin is `GET /cli/tunnel/subdomains`, which returns the
    /// identical `{primary, targets}` payload
    /// ([`tunnel_subdomains_snapshot`] builds both).
    ///
    /// Wire: `{ "kind": "tunnel_subdomains_changed", "primary": string,
    /// "targets": { "<label>": { "target": "<host:port>",
    /// "projectId": string|null }, ... } }`. `primary` is the tunnel's
    /// primary subdomain label (may be empty when unknown); each target
    /// carries the internal endpoint plus the attributed workspace's
    /// `projects.id` (null = unattributed). Pre-0074 daemons sent bare
    /// `"<host:port>"` strings — consumers normalize both shapes.
    TunnelSubdomainsChanged {
        primary: String,
        targets: HashMap<String, SubdomainTargetWire>,
    },

    /// #676 — a tab title was set daemon-side. WORKSPACE-SCOPED.
    /// Push counterpart to the new `POST /cli/workspace/set-tab-title`
    /// route + the daemon-canonical `tab_titles` store. Replaces the
    /// renderer-local-only `setTabTitle` so a rename in window A shows
    /// up in window B + the mobile companion.
    ///
    /// Wire: `{ "kind": "tab_title_changed", "workspacePath": string,
    /// "project": string, "tabId": string, "title": string,
    /// "locked": bool }`. `locked` distinguishes a USER rename (sticky —
    /// receivers must not let PTY/OSC titles clobber it) from an auto
    /// title; it was missing pre-0.40.38, so remote receivers applied
    /// renames unlocked and the next PTY title overwrote them.
    /// `workspacePath` is the project path used by the prefix filter;
    /// `project` is the same value echoed under the contract name the
    /// route request uses (kept distinct so the filter field can't
    /// drift from the addressing field).
    TabTitleChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        project: String,
        #[serde(rename = "tabId")]
        tab_id: String,
        title: String,
        locked: bool,
    },

    /// #677.3 — workspace tab-order persistence advanced. WORKSPACE-
    /// SCOPED. Carries the monotonic `revision` the daemon stamped on
    /// the write so concurrent clients resolve last-write-wins
    /// deterministically: a renderer drops a stale local write whose
    /// base revision is behind this one.
    ///
    /// Wire: `{ "kind": "tab_order_changed", "workspacePath": string,
    /// "project": string, "workspace": string, "revision": number }`.
    /// `project` + `workspace` are the `(project_id, workspace_id)` key
    /// of the `workspace_layouts` row; `workspacePath` is the project
    /// path used by the prefix filter.
    TabOrderChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        project: String,
        workspace: String,
        revision: i64,
    },

    /// #677.1 — a heartbeat session's live (PTY-attached) state flipped.
    /// WORKSPACE-SCOPED. The daemon owns the PTY lifecycle; this fires
    /// when a heartbeat's `active_terminal_id` is stamped (live=true on
    /// wake/spawn) or nulled (live=false on PTY exit). Lets every client
    /// converge the heartbeat live-dot without polling.
    ///
    /// Wire: `{ "kind": "heartbeat_state_changed", "workspacePath":
    /// string, "project": string, "agent": string, "live": bool }`.
    /// `project` is the project_id; `agent` is the heartbeat name;
    /// `workspacePath` is the project path used by the prefix filter
    /// (empty string when the spawn path doesn't have it resolved — the
    /// renderer then matches on `project`).
    HeartbeatStateChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        project: String,
        agent: String,
        live: bool,
    },

    /// Heartbeat-drawer live-update fix — a project's heartbeat ROSTER
    /// mutated (a row was added / removed / archived / unarchived /
    /// enabled / disabled / edited / renamed / delivery re-targeted).
    /// WORKSPACE-SCOPED. Emitted after every SUCCESSFUL heartbeat CRUD
    /// mutation in `heartbeat_routes::dispatch_get` so a list rendered in
    /// another client (sidebar drawer, Settings) converges without a
    /// revisit. Deliberately carries NO row data (the ProjectsChanged
    /// convention): it's a "re-fetch your list" nudge that stays honest
    /// for any mutation source. Live (PTY) flips are NOT roster changes —
    /// those keep riding [`SessionEvent::HeartbeatStateChanged`].
    ///
    /// Wire: `{ "kind": "heartbeat_roster_changed", "workspacePath":
    /// string, "projectId": string }`. `workspacePath` is the project
    /// path used by the prefix filter; `projectId` may be empty when the
    /// emit site couldn't resolve it (consumers then rely on the path
    /// filter alone).
    HeartbeatRosterChanged {
        #[serde(rename = "workspacePath")]
        workspace_path: String,
        #[serde(rename = "projectId")]
        project_id: String,
    },

    /// 0.39.45 (GH #18/#26) — the registered project set changed (a
    /// project was added, removed, or re-registered). APP-LEVEL. The
    /// renderer re-fetches the project list on receipt, so a workspace
    /// added in another window, by the CLI (`k2so workspace create`),
    /// or by an onboarding flow appears in the nav/Settings WITHOUT a
    /// manual window reload. Pre-0.39.45 the only signal was the local
    /// Tauri `sync:projects` event, which never fires for CLI/remote
    /// mutations (and doesn't reach K2 Connect clients at all).
    ///
    /// Wire: `{ "kind": "projects_changed" }`. Deliberately payload-
    /// free — consumers re-fetch the canonical list rather than
    /// patching from a diff.
    ProjectsChanged {},

    /// 0.40.38 — chat-history list mutated (session rename/pin/refresh).
    /// APP-LEVEL refetch signal (no payload — ProjectsChanged convention).
    /// Mirrored from HookEvent::SyncChatHistory / ChatRefreshed in
    /// events.rs so remote clients see chat-list changes live (they
    /// previously rode only the loopback /events bus).
    /// Wire: `{ "kind": "chat_history_changed" }`.
    ChatHistoryChanged {},

    /// S1 (presence/multiplayer arc) — the connected-users roster
    /// changed (a `/cli/sessions/events` socket registered or
    /// deregistered, or — from S4 on — an edit grant toggled).
    /// APP-LEVEL: forwarded to EVERY subscriber regardless of `?path=`
    /// (see `event_matches_workspace`). Carries the WHOLE aggregated
    /// per-user set, not a diff, so client convergence is last-write-
    /// wins on a snapshot (the ActiveChanged convention). The snapshot
    /// twin is `GET /cli/presence/roster` (fetched on `hello`), which
    /// returns the identical `{ "roster": [...] }` payload.
    ///
    /// Wire: `{ "kind": "presence_changed", "roster": [ { "user":
    /// "owner"|username, "role": "owner"|"admin"|"member",
    /// "windowCount": number, "workspaces": string[], "grantedEdit":
    /// bool, "connectedAt": number } ] }` — row shape frozen in
    /// `presence::RosterUser`.
    PresenceChanged {
        roster: Vec<crate::presence::RosterUser>,
    },

    /// 0.40.34 — a process on the daemon's machine asked to open a URL
    /// (the `~/.k2/bin/k2-open` shim intercepting `xdg-open`/`$BROWSER`,
    /// or the `/cli/fs/open-external` route for an http(s) target).
    /// APP-LEVEL: forwarded to EVERY `/cli/sessions/events` subscriber
    /// regardless of `?path=` (see `event_matches_workspace`) so the
    /// CONNECTED app — local or across the K2 Connect tunnel — can open
    /// the URL in a browser tab instead of the daemon's (possibly
    /// headless) machine launching anything.
    ///
    /// Wire: `{ "kind": "open_url", "url": string, "source": string }`.
    /// `url` is validated at the emit sites (http/https only, ≤2048
    /// chars). `source` says who asked: `"shim"` (the staged k2-open
    /// shim via `POST /cli/browser/open-url`) or `"terminal-link"`
    /// (the `/cli/fs/open-external` route). Clients may ignore it.
    OpenUrl { url: String, source: String },

    /// Remote live-update fix — SOMETHING about the project-group set
    /// changed (membership, PoC, structure, layout, or a chat message).
    /// APP-LEVEL twin of the legacy `project-group:*` HookEvents, which
    /// ride the loopback-only `/events` WS → Tauri bus and therefore
    /// never reach a K2 Connect client. Emitted ADDITIVELY alongside
    /// every project-group HookEvent (see `project_group_routes.rs::emit`)
    /// so a remote renderer's Projects page/badge refetches live.
    ///
    /// Wire: `{ "kind": "project_groups_changed", "reason": string }`.
    /// `reason` is the legacy hook name minus its `project-group:`
    /// prefix — one of `groups-changed` | `members-changed` |
    /// `poc-changed` | `layout-changed` | `message-created` — so the
    /// consumer can keep per-event behavior. Deliberately payload-lean
    /// otherwise: it's a refetch signal, consumers re-fetch canonical
    /// truth rather than patching from a diff (the ProjectsChanged
    /// convention).
    ProjectGroupsChanged { reason: String },

    /// Remote live-update fix — the feedback set changed (item created /
    /// answered / status flipped / comment stored). APP-LEVEL twin of the
    /// legacy `feedback:*` HookEvents (loopback-only bus, same story as
    /// [`SessionEvent::ProjectGroupsChanged`]). Emitted ADDITIVELY
    /// alongside every feedback HookEvent (see `feedback_routes.rs::emit`).
    ///
    /// Wire: `{ "kind": "feedback_changed", "reason": string }` with
    /// `reason` = the legacy hook name minus its `feedback:` prefix —
    /// one of `created` | `answered` | `status-changed` | `commented`.
    /// Refetch signal only, no item payload.
    FeedbackChanged { reason: String },
}

/// One nested-subdomain target on the wire (0074): the internal endpoint
/// the label routes to plus the attributed workspace's `projects.id`
/// (`null` = unattributed — the label exists on the control plane but no
/// workspace has created/claimed it through this daemon). Serialized
/// identically inside `GET /cli/tunnel/subdomains` and the
/// [`SessionEvent::TunnelSubdomainsChanged`] broadcast — one shape, two
/// transports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubdomainTargetWire {
    /// Internal target endpoint, e.g. `localhost:3000`.
    pub target: String,
    /// Attributed workspace (`projects.id`), `null` when unattributed.
    #[serde(rename = "projectId")]
    pub project_id: Option<String>,
}

/// The canonical `{primary, targets}` payload BOTH surfaces serve: the
/// cached control-plane routing map (`tunnel::subdomains::current()`)
/// overlaid with the daemon-local 0074 attribution table. Building it in
/// ONE place is what keeps `GET /cli/tunnel/subdomains` and the
/// `tunnel_subdomains_changed` broadcast from ever drifting apart.
/// Attribution rows for labels that are NOT in the routing map are
/// ignored (stale claims for removed labels don't invent rows).
pub fn tunnel_subdomains_snapshot() -> (String, HashMap<String, SubdomainTargetWire>) {
    let map = k2_core::tunnel::subdomains::current();
    let attribution = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::SubdomainWorkspace::map(&conn).unwrap_or_default()
    };
    let targets = map
        .targets
        .iter()
        .map(|(label, target)| {
            (
                label.clone(),
                SubdomainTargetWire {
                    target: target.clone(),
                    project_id: attribution.get(label).cloned(),
                },
            )
        })
        .collect();
    (map.primary.clone(), targets)
}

/// Emit the app-level `tunnel_subdomains_changed` broadcast carrying the
/// current canonical snapshot. Called from the claim/unclaim routes
/// (attribution changed, map identical) — the refresh-loop path arrives
/// via the `HookEvent::TunnelSubdomainsChanged` mirror in `events.rs`,
/// which serializes the same snapshot.
pub fn emit_tunnel_subdomains_changed() {
    let (primary, targets) = tunnel_subdomains_snapshot();
    let _ = emit(SessionEvent::TunnelSubdomainsChanged { primary, targets });
}

static SENDER: OnceLock<broadcast::Sender<SessionEvent>> = OnceLock::new();

/// Observability (Phase B) — the latest `AgentStatusChanged` per `paneId`
/// (== terminal id == the daemon-side session id), with the unix-seconds
/// timestamp we observed it. This is a **memoization of the same truth the
/// `/cli/sessions/events` stream carries, not a parallel one**: it is
/// written ONLY from inside [`emit`] (the single chokepoint every
/// `AgentStatusChanged` flows through — `events.rs::DaemonBroadcastSink`),
/// so a reader of this cache and a subscriber of the bus are fed by the
/// identical event. `/cli/ops/overview` reads it so a pane can render each
/// agent's working/idle status in one round-trip without a live WS.
///
/// Bounded by the number of distinct panes the daemon has seen since boot;
/// entries are overwritten in place. No eviction — a `paneId` count is the
/// live-tab count, which stays small.
static AGENT_STATUS: OnceLock<Mutex<HashMap<String, (String, i64)>>> = OnceLock::new();

fn agent_status_map() -> &'static Mutex<HashMap<String, (String, i64)>> {
    AGENT_STATUS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn unix_now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Record the latest agent status for a pane. Called from [`emit`] on every
/// `AgentStatusChanged` so the cache can never drift from the bus.
fn record_agent_status(pane_id: &str, status: &str) {
    if pane_id.is_empty() {
        return;
    }
    if let Ok(mut map) = agent_status_map().lock() {
        map.insert(pane_id.to_string(), (status.to_string(), unix_now_secs()));
    }
}

/// Observability read accessor — the latest cached `(status, observed_at)`
/// for a pane/session id, or `None` if no `AgentStatusChanged` has been seen
/// for it since boot. `status` is the raw canonical bucket the bus carries
/// (`start` | `stop` | `permission`); callers normalize for display. Same
/// source of truth as `/cli/sessions/events` (see [`AGENT_STATUS`]).
pub fn agent_status_for(pane_id: &str) -> Option<(String, i64)> {
    agent_status_map().lock().ok()?.get(pane_id).cloned()
}

/// Test-only — drop the agent-status cache so the global map doesn't leak
/// between cases.
#[cfg(test)]
pub fn clear_agent_status_for_tests() {
    if let Ok(mut map) = agent_status_map().lock() {
        map.clear();
    }
}

/// Lazy accessor for the broadcast sender. First caller creates the
/// channel. Cheap — the OnceLock is a single atomic load on the hot
/// path.
pub fn sender() -> &'static broadcast::Sender<SessionEvent> {
    SENDER.get_or_init(|| broadcast::channel(EVENT_CHANNEL_CAP).0)
}

/// Take a fresh `Receiver`. One per WS subscriber. Drops cleanly
/// when the subscriber disconnects.
pub fn subscribe() -> broadcast::Receiver<SessionEvent> {
    sender().subscribe()
}

/// Best-effort emit. Returns `Ok(n)` with the number of subscribers
/// the event was queued for; `Err` when there are zero subscribers
/// (treated as a no-op by callers via `let _ =`). The emit cost is
/// bounded — broadcast doesn't synchronize beyond a single atomic
/// per subscriber.
pub fn emit(event: SessionEvent) -> Result<usize, broadcast::error::SendError<SessionEvent>> {
    // Observability (Phase B): memoize the latest agent status off the SAME
    // event that's about to hit the bus, so `/cli/ops/overview` and a
    // `/cli/sessions/events` subscriber read one truth. This is the only
    // writer of AGENT_STATUS.
    if let SessionEvent::AgentStatusChanged { pane_id, status, .. } = &event {
        record_agent_status(pane_id, status);
    }
    sender().send(event)
}

/// 0.39.39 (#677.1) — best-effort broadcast that a heartbeat session's
/// live (PTY-attached) state flipped. Call with `live=true` right after
/// stamping `active_terminal_id` on a heartbeat spawn, and `live=false`
/// when the PTY exits (the unregister chokepoint does the latter). The
/// broadcast `let _ =`-swallows the no-subscribers case. `workspace_path`
/// may be empty when the spawn path doesn't have it resolved — the
/// renderer then matches on `project` (the project_id).
pub fn emit_heartbeat_live(workspace_path: &str, project_id: &str, agent: &str, live: bool) {
    let _ = emit(SessionEvent::HeartbeatStateChanged {
        workspace_path: workspace_path.to_string(),
        project: project_id.to_string(),
        agent: agent.to_string(),
        live,
    });
}

/// Heartbeat-drawer live-update fix — best-effort broadcast that a
/// project's heartbeat roster mutated (a CRUD change, NOT a live flip).
/// Call right after the mutation SUCCEEDS; consumers re-fetch the
/// canonical list. The broadcast `let _ =`-swallows the no-subscribers
/// case. `project_id` may be empty when the caller couldn't resolve it —
/// the WS path filter still routes the nudge on `workspace_path`.
pub fn emit_heartbeat_roster_changed(workspace_path: &str, project_id: &str) {
    let _ = emit(SessionEvent::HeartbeatRosterChanged {
        workspace_path: workspace_path.to_string(),
        project_id: project_id.to_string(),
    });
}

/// Extract `tab-<paneGroupId>` from a `v2_session_map` agent_name.
/// Returns `None` for non-tab-shaped keys (pinned chat = bare UUID,
/// heartbeats = bare workspace key, legacy `<pid>:<agent>` shapes).
/// The renderer uses this to decide whether to surface the event as
/// a new tab — non-tab events are forwarded so the mobile companion
/// can see the full inventory without the renderer adopting them.
pub fn pane_group_id_from_agent(agent_name: &str) -> Option<String> {
    agent_name
        .strip_prefix("tab-")
        .map(|rest| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// FROZEN WIRE CONTRACT (task #672): the renderer codes against
    /// EXACTLY `{ "kind": "active_changed", "activeProjectIds":
    /// string[], "activeWindowHours": number }`. Pin the serialized
    /// shape so any field rename / tag drift fails CI loudly.
    #[test]
    fn active_changed_serializes_to_frozen_contract() {
        let ev = SessionEvent::ActiveChanged {
            active_project_ids: vec!["pid-a".to_string(), "pid-b".to_string()],
            active_window_hours: 24,
        };
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&ev).unwrap()).unwrap();
        assert_eq!(json["kind"], "active_changed");
        assert_eq!(
            json["activeProjectIds"],
            serde_json::json!(["pid-a", "pid-b"])
        );
        assert_eq!(json["activeWindowHours"], 24);
        // No stray snake_case leakage.
        assert!(json.get("active_project_ids").is_none());
        assert!(json.get("active_window_hours").is_none());
        // Exactly the three documented keys.
        let obj = json.as_object().unwrap();
        assert_eq!(obj.len(), 3, "unexpected extra fields: {obj:?}");
    }

    // ── 0.39.39 (#675/#676/#677) FROZEN WIRE CONTRACTS ──────────────
    // Each test pins the EXACT serialized `kind` tag + the EXACT field
    // key set the Wave-C renderer consumers code against. A field rename
    // or tag drift fails CI loudly.

    fn as_json(ev: &SessionEvent) -> serde_json::Value {
        serde_json::from_str(&serde_json::to_string(ev).unwrap()).unwrap()
    }

    fn assert_keys(json: &serde_json::Value, keys: &[&str]) {
        let obj = json.as_object().unwrap();
        for k in keys {
            assert!(obj.contains_key(*k), "missing key {k} in {obj:?}");
        }
        assert_eq!(
            obj.len(),
            keys.len(),
            "unexpected key set: {obj:?} (expected exactly {keys:?})"
        );
    }

    #[test]
    fn llm_status_changed_frozen_contract() {
        let json = as_json(&SessionEvent::LlmStatusChanged {
            loaded: true,
            model_path: Some("/m/q.gguf".into()),
            downloading: false,
            download_percent: Some(42.5),
        });
        assert_eq!(json["kind"], "llm_status_changed");
        assert_eq!(json["loaded"], true);
        assert_eq!(json["modelPath"], "/m/q.gguf");
        assert_eq!(json["downloading"], false);
        assert_eq!(json["downloadPercent"], 42.5);
        assert_keys(&json, &["kind", "loaded", "modelPath", "downloading", "downloadPercent"]);
        // null model_path + percent serialize as JSON null (present key).
        let json2 = as_json(&SessionEvent::LlmStatusChanged {
            loaded: false,
            model_path: None,
            downloading: true,
            download_percent: None,
        });
        assert!(json2["modelPath"].is_null());
        assert!(json2["downloadPercent"].is_null());
    }

    #[test]
    fn agent_status_changed_frozen_contract() {
        let json = as_json(&SessionEvent::AgentStatusChanged {
            pane_id: "term-1".into(),
            tab_id: "tab-1".into(),
            status: "start".into(),
        });
        assert_eq!(json["kind"], "agent_status_changed");
        assert_eq!(json["paneId"], "term-1");
        assert_eq!(json["tabId"], "tab-1");
        assert_eq!(json["status"], "start");
        assert_keys(&json, &["kind", "paneId", "tabId", "status"]);
    }

    #[test]
    fn review_queue_changed_frozen_contract() {
        let json = as_json(&SessionEvent::ReviewQueueChanged {
            workspace_path: "/x/foo".into(),
        });
        assert_eq!(json["kind"], "review_queue_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_keys(&json, &["kind", "workspacePath"]);
    }

    #[test]
    fn review_changed_frozen_contract() {
        let json = as_json(&SessionEvent::ReviewChanged {
            workspace_path: "/x/foo".into(),
            agent: Some("scout".into()),
        });
        assert_eq!(json["kind"], "review_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["agent"], "scout");
        assert_keys(&json, &["kind", "workspacePath", "agent"]);
        let json2 = as_json(&SessionEvent::ReviewChanged {
            workspace_path: "/x/foo".into(),
            agent: None,
        });
        assert!(json2["agent"].is_null());
    }

    #[test]
    fn tunnel_status_changed_frozen_contract() {
        let json = as_json(&SessionEvent::TunnelStatusChanged {
            running: true,
            public_url: Some("https://rosson.k2.dev".into()),
        });
        assert_eq!(json["kind"], "tunnel_status_changed");
        assert_eq!(json["running"], true);
        assert_eq!(json["publicUrl"], "https://rosson.k2.dev");
        assert_keys(&json, &["kind", "running", "publicUrl"]);
    }

    /// FROZEN WIRE CONTRACT (URLs drawer + K2 Connect settings): the
    /// renderer codes against EXACTLY `{ "kind":
    /// "tunnel_subdomains_changed", "primary": string, "targets":
    /// { label: { "target": string, "projectId": string|null } } }` —
    /// the same shape `GET /cli/tunnel/subdomains` returns (0074: each
    /// target is an object carrying the workspace attribution, not the
    /// bare endpoint string it was pre-release). Pin it so a rename
    /// fails CI loudly.
    #[test]
    fn tunnel_subdomains_changed_frozen_contract() {
        let mut targets = HashMap::new();
        targets.insert(
            "staging".to_string(),
            SubdomainTargetWire {
                target: "localhost:3000".to_string(),
                project_id: Some("proj-uuid".to_string()),
            },
        );
        targets.insert(
            "preview".to_string(),
            SubdomainTargetWire {
                target: "127.0.0.1:8080".to_string(),
                project_id: None,
            },
        );
        let json = as_json(&SessionEvent::TunnelSubdomainsChanged {
            primary: "rosson".into(),
            targets,
        });
        assert_eq!(json["kind"], "tunnel_subdomains_changed");
        assert_eq!(json["primary"], "rosson");
        assert_eq!(json["targets"]["staging"]["target"], "localhost:3000");
        assert_eq!(json["targets"]["staging"]["projectId"], "proj-uuid");
        assert_eq!(json["targets"]["preview"]["target"], "127.0.0.1:8080");
        // Unattributed serializes as an EXPLICIT null, never a missing key.
        assert!(json["targets"]["preview"]["projectId"].is_null());
        assert!(json["targets"]["preview"]
            .as_object()
            .unwrap()
            .contains_key("projectId"));
        assert_eq!(json["targets"].as_object().unwrap().len(), 2);
        assert_keys(&json, &["kind", "primary", "targets"]);
        // An empty map serializes as an empty object (present key), so the
        // renderer's "no nested URLs" empty state never sees `undefined`.
        let json2 = as_json(&SessionEvent::TunnelSubdomainsChanged {
            primary: String::new(),
            targets: HashMap::new(),
        });
        assert_eq!(json2["primary"], "");
        assert!(json2["targets"].as_object().unwrap().is_empty());
    }

    /// [`tunnel_subdomains_snapshot`] is the ONE builder both the GET
    /// route and the broadcast serialize: the cached routing map overlaid
    /// with the 0074 attribution table. Attributed labels carry their
    /// project id, unattributed carry None, and stale attribution rows
    /// (labels absent from the routing map) invent nothing. Uses a
    /// sentinel primary + labels so parallel tests sharing the
    /// process-global caches can't false-positive.
    #[test]
    fn tunnel_subdomains_snapshot_overlays_attribution() {
        let pid = std::process::id();
        let claimed = format!("snap-claimed-{pid}");
        let bare = format!("snap-bare-{pid}");
        let stale = format!("snap-stale-{pid}");

        let db = k2_core::db::shared();
        {
            let conn = db.lock();
            k2_core::db::schema::SubdomainWorkspace::claim(&conn, &claimed, "proj-snap")
                .expect("claim");
            k2_core::db::schema::SubdomainWorkspace::claim(&conn, &stale, "proj-stale")
                .expect("claim stale");
        }
        let mut map_targets = std::collections::HashMap::new();
        map_targets.insert(claimed.clone(), "localhost:3000".to_string());
        map_targets.insert(bare.clone(), "localhost:4000".to_string());
        k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap {
            primary: format!("snap-primary-{pid}"),
            targets: map_targets,
        });

        let (primary, targets) = tunnel_subdomains_snapshot();
        assert_eq!(primary, format!("snap-primary-{pid}"));
        assert_eq!(
            targets.get(&claimed),
            Some(&SubdomainTargetWire {
                target: "localhost:3000".to_string(),
                project_id: Some("proj-snap".to_string()),
            })
        );
        assert_eq!(
            targets.get(&bare),
            Some(&SubdomainTargetWire {
                target: "localhost:4000".to_string(),
                project_id: None,
            })
        );
        assert!(
            !targets.contains_key(&stale),
            "an attribution row for a label outside the routing map must not invent a target"
        );

        // Clean up the shared in-memory DB + process-global map cache.
        {
            let conn = db.lock();
            let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, &claimed);
            let _ = k2_core::db::schema::SubdomainWorkspace::unclaim(&conn, &stale);
        }
        k2_core::tunnel::subdomains::store(Default::default());
    }

    #[test]
    fn tab_title_changed_frozen_contract() {
        let json = as_json(&SessionEvent::TabTitleChanged {
            workspace_path: "/x/foo".into(),
            project: "proj-uuid".into(),
            tab_id: "tab-7".into(),
            title: "My Tab".into(),
            locked: true,
        });
        assert_eq!(json["kind"], "tab_title_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["project"], "proj-uuid");
        assert_eq!(json["tabId"], "tab-7");
        assert_eq!(json["title"], "My Tab");
        assert_eq!(json["locked"], true);
        assert_keys(&json, &["kind", "workspacePath", "project", "tabId", "title", "locked"]);
    }

    #[test]
    fn tab_order_changed_frozen_contract() {
        let json = as_json(&SessionEvent::TabOrderChanged {
            workspace_path: "/x/foo".into(),
            project: "proj-uuid".into(),
            workspace: "ws-uuid".into(),
            revision: 5,
        });
        assert_eq!(json["kind"], "tab_order_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["project"], "proj-uuid");
        assert_eq!(json["workspace"], "ws-uuid");
        assert_eq!(json["revision"], 5);
        assert_keys(&json, &["kind", "workspacePath", "project", "workspace", "revision"]);
    }

    #[test]
    fn heartbeat_state_changed_frozen_contract() {
        let json = as_json(&SessionEvent::HeartbeatStateChanged {
            workspace_path: "/x/foo".into(),
            project: "proj-uuid".into(),
            agent: "nightly".into(),
            live: true,
        });
        assert_eq!(json["kind"], "heartbeat_state_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["project"], "proj-uuid");
        assert_eq!(json["agent"], "nightly");
        assert_eq!(json["live"], true);
        assert_keys(&json, &["kind", "workspacePath", "project", "agent", "live"]);
    }

    /// FROZEN WIRE CONTRACT (heartbeat-drawer live-update fix): the
    /// renderer codes against EXACTLY `{ "kind":
    /// "heartbeat_roster_changed", "workspacePath": string, "projectId":
    /// string }`. Pin the serialized shape so any rename fails CI loudly.
    #[test]
    fn heartbeat_roster_changed_frozen_contract() {
        let json = as_json(&SessionEvent::HeartbeatRosterChanged {
            workspace_path: "/x/foo".into(),
            project_id: "proj-uuid".into(),
        });
        assert_eq!(json["kind"], "heartbeat_roster_changed");
        assert_eq!(json["workspacePath"], "/x/foo");
        assert_eq!(json["projectId"], "proj-uuid");
        assert_keys(&json, &["kind", "workspacePath", "projectId"]);
    }

    /// Observability (Phase B) no-divergence guard: a single `emit` of an
    /// `AgentStatusChanged` feeds BOTH the broadcast subscriber (what
    /// `/cli/sessions/events` delivers) AND the `AGENT_STATUS` cache (what
    /// `/cli/ops/overview` reads). They must carry the IDENTICAL status —
    /// there is one writer (`emit`), so the cache can never drift from the
    /// stream.
    #[tokio::test(flavor = "current_thread")]
    async fn agent_status_cache_matches_broadcast_truth() {
        clear_agent_status_for_tests();
        let pane = format!(
            "ops-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let mut rx = subscribe();
        let _ = emit(SessionEvent::AgentStatusChanged {
            pane_id: pane.clone(),
            tab_id: "tab-x".into(),
            status: "start".into(),
        });

        // Drain until our probe arrives (the global bus may carry other
        // tests' events). This is what a `/cli/sessions/events` subscriber
        // would receive.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let stream_status = loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("did not receive probe AgentStatusChanged in time");
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(SessionEvent::AgentStatusChanged { pane_id, status, .. }))
                    if pane_id == pane =>
                {
                    break status;
                }
                Ok(Ok(_)) => continue, // contamination from another test
                Ok(Err(_)) => panic!("receiver closed"),
                Err(_) => panic!("timed out waiting for probe event"),
            }
        };

        // The cache the overview reads must hold the SAME status.
        let (cached_status, observed_at) =
            agent_status_for(&pane).expect("cache populated by the same emit");
        assert_eq!(
            cached_status, stream_status,
            "overview cache must match the broadcast truth (no divergent source)"
        );
        assert_eq!(cached_status, "start");
        assert!(observed_at > 0, "observed_at timestamp must be stamped");

        // A pane that never emitted has no cached status.
        assert!(agent_status_for("ops-never-emitted").is_none());
        clear_agent_status_for_tests();
    }

    #[test]
    fn pane_group_id_extracts_tab_prefix() {
        assert_eq!(
            pane_group_id_from_agent("tab-abc123"),
            Some("abc123".to_string()),
        );
        assert_eq!(
            pane_group_id_from_agent("tab-uuid-with-hyphens-here"),
            Some("uuid-with-hyphens-here".to_string()),
        );
    }

    #[test]
    fn pane_group_id_is_none_for_non_tab_agents() {
        assert_eq!(pane_group_id_from_agent(""), None);
        assert_eq!(
            pane_group_id_from_agent("c3c5b9d6-e123-4456-89ab-cdef01234567"),
            None,
        );
        assert_eq!(pane_group_id_from_agent("__lead__"), None);
        assert_eq!(
            pane_group_id_from_agent("project-id:agent-name"),
            None,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn emit_with_no_subscribers_is_a_no_op() {
        // Fresh OnceLock is fine for this test only because no other
        // subscribers exist in this process at this moment. Best-effort
        // assertion: emit returns Err (zero subscribers) and we don't
        // care.
        let res = emit(SessionEvent::SessionRemoved {
            workspace_path: "/tmp".into(),
            pane_group_id: None,
            agent_name: "test-no-subs".into(),
        });
        // Either Err (no subs) or Ok(0+) — both fine; emitter callers
        // never inspect the result.
        let _ = res;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_emitted_event() {
        // The broadcast channel is process-global, so concurrent
        // tests (especially `register`/`unregister` integrations on
        // a different test thread) can leak events into our
        // receiver. Use a unique session_id as a probe and drain
        // events until we find the one we emitted — anything else
        // is contamination and gets ignored.
        let probe_id = format!(
            "test-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let mut rx = subscribe();
        let _ = emit(SessionEvent::SessionAdded {
            workspace_path: "/x/foo".into(),
            pane_group_id: Some("pg-1".into()),
            agent_name: "tab-pg-1".into(),
            command: Some("zsh".into()),
            args: vec![],
            session_id: probe_id.clone(),
            is_v2: true,
            sandbox_backend: None,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("did not receive probe event in time");
            }
            let got = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(_)) => panic!("receiver closed"),
                Err(_) => panic!("timed out waiting for probe event"),
            };
            if let SessionEvent::SessionAdded { ref session_id, .. } = got {
                if session_id == &probe_id {
                    match got {
                        SessionEvent::SessionAdded { workspace_path, pane_group_id, agent_name, .. } => {
                            assert_eq!(workspace_path, "/x/foo");
                            assert_eq!(pane_group_id.as_deref(), Some("pg-1"));
                            assert_eq!(agent_name, "tab-pg-1");
                        }
                        _ => unreachable!(),
                    }
                    break;
                }
            }
            // Otherwise: contamination from another test — keep
            // draining.
        }
    }

    #[test]
    fn workspace_path_filter_rules_match_cli_endpoint() {
        // Document the filter rule in a test. The actual filter is
        // applied in session_events_ws.rs's per-subscriber loop;
        // this test just pins the rule.
        fn matches(cwd: &str, workspace_path: &str) -> bool {
            let trimmed = workspace_path.trim_end_matches('/');
            let prefix_with_slash = if trimmed.is_empty() {
                "/".to_string()
            } else {
                format!("{}/", trimmed)
            };
            let cwd_trim = cwd.trim_end_matches('/');
            cwd_trim == trimmed || cwd.starts_with(&prefix_with_slash)
        }
        assert!(matches("/x/K2SO", "/x/K2SO"));
        assert!(matches("/x/K2SO/sub", "/x/K2SO"));
        assert!(matches("/x/K2SO/sub/deeper", "/x/K2SO"));
        // Sibling — must NOT match (the exact bug `list-for-workspace`
        // got bitten by before).
        assert!(!matches("/x/K2SO-website", "/x/K2SO"));
        assert!(!matches("/x/other", "/x/K2SO"));
        // Trailing-slash tolerance.
        assert!(matches("/x/K2SO/", "/x/K2SO"));
        assert!(matches("/x/K2SO", "/x/K2SO/"));
    }

    /// S1 presence FROZEN WIRE CONTRACT: `{ "kind": "presence_changed",
    /// "roster": [...] }` with the RosterUser row shape (camelCase field
    /// names). The S2 renderer store + the `GET /cli/presence/roster`
    /// snapshot both code against exactly this.
    #[test]
    fn presence_changed_frozen_contract() {
        let json = as_json(&SessionEvent::PresenceChanged {
            roster: vec![crate::presence::RosterUser {
                user: "owner".into(),
                role: "owner".into(),
                window_count: 2,
                workspaces: vec!["/x/foo".into()],
                granted_edit: false,
                connected_at: 1_700_000_000,
            }],
        });
        assert_eq!(json["kind"], "presence_changed");
        assert_keys(&json, &["kind", "roster"]);
        let row = &json["roster"][0];
        assert_eq!(row["user"], "owner");
        assert_eq!(row["role"], "owner");
        assert_eq!(row["windowCount"], 2);
        assert_eq!(row["workspaces"], serde_json::json!(["/x/foo"]));
        assert_eq!(row["grantedEdit"], false);
        assert_eq!(row["connectedAt"], 1_700_000_000i64);
        let row_obj = row.as_object().unwrap();
        assert_eq!(row_obj.len(), 6, "unexpected roster row keys: {row_obj:?}");
    }

    /// 0.40.34 FROZEN WIRE CONTRACT: `{ "kind": "open_url", "url":
    /// string, "source": string }`. The renderer's browser-tab consumer
    /// codes against exactly these keys; a tag/field drift fails CI
    /// loudly (same discipline as the 0.39.39 events above).
    #[test]
    fn open_url_frozen_contract() {
        let json = as_json(&SessionEvent::OpenUrl {
            url: "https://example.com/docs?q=1".into(),
            source: "shim".into(),
        });
        assert_eq!(json["kind"], "open_url");
        assert_eq!(json["url"], "https://example.com/docs?q=1");
        assert_eq!(json["source"], "shim");
        assert_keys(&json, &["kind", "url", "source"]);
    }

    /// Remote live-update FROZEN WIRE CONTRACT: `{ "kind":
    /// "project_groups_changed", "reason": string }`. The renderer's
    /// `stores/project-groups.ts` consumer switches on `reason` (the
    /// legacy hook name minus its `project-group:` prefix); a tag/field
    /// drift fails CI loudly (the open_url discipline).
    #[test]
    fn project_groups_changed_frozen_contract() {
        let json = as_json(&SessionEvent::ProjectGroupsChanged {
            reason: "members-changed".into(),
        });
        assert_eq!(json["kind"], "project_groups_changed");
        assert_eq!(json["reason"], "members-changed");
        assert_keys(&json, &["kind", "reason"]);
    }

    /// Remote live-update FROZEN WIRE CONTRACT: `{ "kind":
    /// "feedback_changed", "reason": string }`. The renderer's
    /// `stores/feedback.ts` consumer switches on `reason` (the legacy
    /// hook name minus its `feedback:` prefix).
    #[test]
    fn feedback_changed_frozen_contract() {
        let json = as_json(&SessionEvent::FeedbackChanged {
            reason: "created".into(),
        });
        assert_eq!(json["kind"], "feedback_changed");
        assert_eq!(json["reason"], "created");
        assert_keys(&json, &["kind", "reason"]);
    }

    /// P3c (D1) FROZEN WIRE CONTRACT: a `SessionAdded` for a REAL sandbox cell
    /// carries `sandbox_backend: "microvm"` so the renderer's generic
    /// tab-adoption consumer can light the D9 orange marker. A passthrough /
    /// bare-PTY session omits the key entirely (`skip_serializing_if = None`),
    /// keeping a normal `SessionAdded` byte-identical to pre-P3c (older clients
    /// + the default path see no new field).
    #[test]
    fn session_added_sandbox_backend_serialization_contract() {
        // Real sandbox cell → the key is present and equals "microvm".
        let sandboxed = as_json(&SessionEvent::SessionAdded {
            workspace_path: "/home/u/.k2/sandbox-sessions/abc".into(),
            pane_group_id: None,
            agent_name: "api-owner-uuid".into(),
            command: Some("claude".into()),
            args: vec!["--dangerously-skip-permissions".into()],
            session_id: "sess-1".into(),
            is_v2: true,
            sandbox_backend: Some("microvm".into()),
        });
        assert_eq!(sandboxed["kind"], "session_added");
        assert_eq!(sandboxed["sandbox_backend"], "microvm");
        assert_eq!(sandboxed["isV2"], true);

        // Passthrough / bare-PTY → the key is OMITTED entirely (default-OFF
        // parity: no new wire key for a normal tab).
        let plain = as_json(&SessionEvent::SessionAdded {
            workspace_path: "/x/foo".into(),
            pane_group_id: Some("pg-1".into()),
            agent_name: "tab-pg-1".into(),
            command: Some("zsh".into()),
            args: vec![],
            session_id: "sess-2".into(),
            is_v2: true,
            sandbox_backend: None,
        });
        assert!(
            plain.get("sandbox_backend").is_none(),
            "a non-sandbox SessionAdded must omit sandbox_backend (byte-identical to pre-P3c): {plain:?}",
        );
    }
}
