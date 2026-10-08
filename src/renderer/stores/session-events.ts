// 0.38.0 Commit 4 — daemon-authoritative session lifecycle subscription.
//
// Opens a long-lived WebSocket to `/cli/sessions/events?path=<workspace>`.
// The daemon pushes JSON frames every time a v2 session is registered or
// removed (and, eventually, renamed). Callers wire handlers per workspace
// to keep the tab store in sync with what the daemon thinks exists,
// replacing the old `sync:tabs` Tauri-event broadcast.
//
// Wire format (matches `crates/k2so-daemon/src/session_events.rs`):
//   { kind: 'hello',           workspace_path, subscriber_id }
//   { kind: 'session_added',   workspace_path, paneGroupId?, agent_name, command?, args, cwd?, isV2 }
//   { kind: 'session_removed', workspace_path, paneGroupId?, agent_name }
//   { kind: 'session_renamed', workspace_path, paneGroupId?, title }
//
// Backoff + auto-reconnect: a clean Close frame from the server is
// treated as "stop trying" (e.g. workspace deauthed); any other drop
// schedules a retry with exponential backoff (500ms → 5s cap). The
// caller's `onHello` handler fires every time the WS reconnects, so
// the tabs store can choose to re-reconcile after a transient drop.
//
// Cleanup: the returned `UnsubscribeFn` closes the socket, cancels
// any pending backoff timer, and prevents further reconnect attempts.

import { getDaemonWs, invalidateDaemonWs, daemonWsBase, type DaemonWsAvailable } from '@/kessel/daemon-ws'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useActiveStore } from '@/stores/active'
import { useConnectHostStore } from '@/stores/connect-host'
import { jittered } from '@/lib/backoff'
import { logRemotePath } from '@/lib/remote-path-log'
import {
  noteRemoteEventsClosed,
  noteRemoteEventsOpened,
} from '@/lib/remote-ws-drop'
import type { ServerScope } from '@/kessel/server-scope'
import { openQueuedWebSocket } from '@/lib/grid-dial-queue'
import { notePoolSocketClose } from '@/lib/pool-hooks'
import { CARRIED_KINDS, dispatchFrom, type DispatchTable } from '@/stores/session-event-kinds'
import { publishThreadAddress } from '@/lib/thread-address-bus'

// ── Wire types ───────────────────────────────────────────────────────────

export interface SessionAddedEvent {
  kind: 'session_added'
  workspace_path: string
  /** Serde renames `pane_group_id` → `paneGroupId` per the snake-vs-camel
   *  convention used elsewhere in the wire schema; we just match what the
   *  daemon emits. */
  pane_group_id: string | null
  agent_name: string
  command: string | null
  args: string[]
  session_id: string
  isV2: boolean
  /** P3c (D1) — the resolved sandbox backend name, present ONLY when the
   *  daemon ran this session under a REAL sandbox (e.g. `'microvm'`); absent
   *  (the field is `skip_serializing_if = None` daemon-side) for the default
   *  passthrough / bare-PTY path. The generic tab-adoption consumer reads it
   *  so an externally / API-spawned cell lights the D9 orange tab marker
   *  immediately, without waiting for the spawn-response echo. Optional +
   *  forward-compatible: older daemons omit it and the renderer treats the
   *  absence as "not sandboxed". */
  sandbox_backend?: string
  /** k2 sidecar SC33 — the daemon-seeded label (a sidecar's Chats name).
   *  Absent when the daemon seeded none; adopt paths fall back to the
   *  command. */
  label?: string
  /** SC33 — the label is locked (PTY titles must not replace it). */
  labelLocked?: boolean
  /** Client-only: Chat history explicit open bypasses hide-sessions. */
  forceAdopt?: boolean
}

export interface SessionRemovedEvent {
  kind: 'session_removed'
  workspace_path: string
  pane_group_id: string | null
  agent_name: string
}

export interface SessionRenamedEvent {
  kind: 'session_renamed'
  workspace_path: string
  pane_group_id: string | null
  title: string
}

export interface HelloEvent {
  kind: 'hello'
  workspace_path: string
  subscriber_id: number
  /** 0.40.48 (optional — older daemons omit it): the daemon's per-boot
   *  instance id, snake_case on the WS wire (`instance_id`; the HTTP
   *  /boot-status carries the camelCase `instanceId`). The primary room's
   *  restart detection is ConnectionGate's boot-status poll; a remote room
   *  reads it here (prd-daemon-activity-and-thread-working-v1 RL4/A28:
   *  `onAppHello` handlers receive this frame, and an id that differs
   *  from the last one seen means a daemon restart, so the activity
   *  snapshot is re-pulled). */
  instance_id?: string
}

/**
 * Canonical daemon-owned Active delta (#672,
 * .k2so/prds/daemon-canonical-active.md §4.4). Broadcast on the SAME
 * session-events bus, but NOT tied to a workspace path — the daemon emits
 * it to every subscriber after any Active recompute (activate / pin /
 * dismiss / window-tick / reap-close). Carries the WHOLE set so client
 * convergence is trivial + order-independent (last-write-wins).
 */
export interface ActiveChangedEvent {
  kind: 'active_changed'
  activeProjectIds: string[]
  activeWindowHours: number
}

// ── Wave B broadcast events (#675/#677) ───────────────────────────────────
//
// These let the renderer DROP its polling intervals and subscribe to the
// daemon's source-of-truth. Wire-frozen against
// `crates/k2so-daemon/src/session_events.rs`. App-level events are
// forwarded to EVERY subscriber regardless of `?path=`; workspace-scoped
// events carry a `workspacePath` and are forwarded only when it matches the
// subscriber's `?path=` (cwd-prefix rule). Field names are camelCase on the
// wire (per-field serde `rename`).

/** APP-LEVEL — local LLM (AI Workspace Assistant) model status changed.
 *  Replaces the `/cli/llm/status` poll in `stores/assistant.ts`. */
export interface LlmStatusChangedEvent {
  kind: 'llm_status_changed'
  loaded: boolean
  modelPath: string | null
  downloading: boolean
  /** `0.0..=100.0` while a download is in flight, `null` otherwise. */
  downloadPercent: number | null
}

/** APP-LEVEL — an agent's working/idle status flipped. Replaces the
 *  list-running + agent-status poll in `stores/active-agents.ts`.
 *  Deprecated compat (prd-daemon-activity-and-thread-working-v1 RL5): the
 *  daemon derives it from its activity row; `activity_changed` replaces
 *  it. */
export interface AgentStatusChangedEvent {
  kind: 'agent_status_changed'
  /** The `K2_PANE_ID` the PTY was spawned with. For a v2 session this is
   *  the daemon **session id**, not the renderer's terminal id (vs-live
   *  Z30): a room maps it through its tabs' `sessionId`. */
  paneId: string
  tabId: string
  /** Canonical bucket: `start` (working) | `stop` (idle) | `permission`. */
  status: 'start' | 'stop' | 'permission'
  /** Daemon-authoritative workspace path (0.40.65+). Remote-safe
   *  attribution for Active-bar / finish toasts. Absent on older daemons. */
  workspacePath?: string | null
}

/** APP-LEVEL (0.40.39) — daemon-side per-session activity transition
 *  (session_activity.rs): Title/Bell-derived, visibility-independent.
 *  Feeds the activity store's daemon-truth map so tab spinners stay
 *  correct for hidden/unmounted panes. */
/** Deprecated compat (prd-daemon-activity-and-thread-working-v1 RL5): from
 *  the daemon's activity row (`monitoring` → working, `waiting` →
 *  permission, `unverifiable` → idle), no longer the title observer.
 *  `activity_changed` replaces it. */
export interface SessionActivityChangedEvent {
  kind: 'session_activity_changed'
  workspacePath: string
  agentName: string
  /** terminalId for tab-spawned sessions ("tab-" stripped daemon-side). */
  paneGroupId: string | null
  status: 'working' | 'idle' | 'permission'
}

/** APP-LEVEL — K2 Connect tunnel connector state transitioned. Replaces
 *  the `/cli/tunnel/status` poll in `CompanionSection.tsx`. */
export interface TunnelStatusChangedEvent {
  kind: 'tunnel_status_changed'
  running: boolean
  publicUrl: string | null
}

/** APP-LEVEL — the tunnel's cached NESTED-subdomain routing map changed
 *  (URLs drawer + K2 Connect settings): the daemon's refresh loop landed
 *  a map that differs from the cached one, OR a label's workspace
 *  attribution changed (0074 claim/unclaim). Carries the WHOLE map (not a
 *  diff) so the consumer converges without a follow-up GET; the snapshot
 *  twin is `GET /cli/tunnel/subdomains`, which returns the identical
 *  `{primary, targets}` payload. `primary` may be empty when the daemon
 *  hasn't learned the tunnel's primary label. */
export interface TunnelSubdomainsChangedEvent {
  kind: 'tunnel_subdomains_changed'
  primary: string
  /** `nested label → { target, projectId }` where `target` is the internal
   *  endpoint (e.g. `localhost:3000`) and `projectId` the attributed
   *  workspace (null = unattributed). Pre-0074 daemons sent the bare
   *  endpoint string — consumers run both shapes through
   *  `normalizeTargets` (urls-ports.ts) rather than trusting the wire. */
  targets: Record<string, { target: string; projectId: string | null } | string>
}

/** APP-LEVEL — a daemon-owned published service changed (spawn / stop /
 *  exit / boot respawn). Carries `projectId` so the workspace Published
 *  drawer can refetch `GET /cli/publish/list` only when it matches (or
 *  refetch anyway — that list is already project-scoped). Same routing
 *  class as `tunnel_subdomains_changed`. */
export interface PublishServicesChangedEvent {
  kind: 'publish_services_changed'
  projectId: string
}

/** APP-LEVEL — a workspace resource was added or removed. Carries
 *  `workspaceId` (`projects.id`) so Files / Projects / Settings refetch
 *  this list without `fetchProjects()`. Same routing class as
 *  `publish_services_changed`. */
export interface WorkspaceResourcesChangedEvent {
  kind: 'workspace_resources_changed'
  workspaceId: string
}

// NOTE (0.40.31): the WORKSPACE-SCOPED review events (`review_queue_changed`,
// `review_changed`) are still broadcast by the daemon (the `k2 review` system
// lives on), but this app no longer consumes them — the Review Queue modal +
// ReviewPanel surfaces were deleted with the 0.40.31 cleanup. Home 0.43.2:
// they are typed so the kind registry (`session-event-kinds.ts`) can mark
// them ignored on purpose instead of letting them read as unknown.

/** WORKSPACE-SCOPED — the review queue changed. Ignored (see above). */
export interface ReviewQueueChangedEvent {
  kind: 'review_queue_changed'
  workspacePath: string
}

/** WORKSPACE-SCOPED — one review changed. Ignored (see above). */
export interface ReviewChangedEvent {
  kind: 'review_changed'
  workspacePath: string
  agent: string | null
}

/** APP-LEVEL — mail state changed (server, domain, an approval asked for or
 *  decided). Refetch signal only. Consumer: Settings → Email
 *  (`onMailChanged`). `reason` is `server-state-changed` |
 *  `domain-status-changed` | `send-approval-requested` | `send-decided`. */
export interface MailChangedEvent {
  kind: 'mail_changed'
  reason: string
}

/** APP-LEVEL — the host's custom-domain inventory changed
 *  (prd-dns-pending-and-cutover-safety-v1 DN12e): attach / remove, a manual
 *  check, the daemon's own pending-zone re-check flipping a zone, or a zone
 *  missing from k2.dev's list. Refetch signal. Consumer: Settings → K2
 *  Server → Domains (`onDomainsChanged`). `reason` is `attached` |
 *  `removed` | `refreshed` | `status_changed` | `zone_missing`; `apex` is
 *  null when several domains changed. */
export interface DomainsChangedEvent {
  kind: 'domains_changed'
  reason: string
  apex: string | null
}

/** APP-LEVEL — a remote drive attempt was refused (owner audit). Ignored:
 *  no client surface shows it yet. Fields are snake_case on the wire. */
export interface RemoteSessionAccessDeniedEvent {
  kind: 'remote_session_access_denied'
  principal_label: string
  reason: string
  code: string
  ts: number
}

/** WORKSPACE-SCOPED — a tab's title changed (daemon-canonical, #676). A
 *  rename in one client/window broadcasts here so every other client
 *  converges its tab BAR label. `project` is the project PATH echoed under
 *  the contract name (kept distinct from the filter field so they can't
 *  drift); `tabId`/`title` are the renamed tab + its new label. */
export interface TabTitleChangedEvent {
  kind: 'tab_title_changed'
  workspacePath: string
  project: string
  tabId: string
  title: string
  /** Tab-rename stickiness — true when this title is a USER rename the
   *  daemon marked locked. Auto-sync paths in the renderer must not
   *  clobber a locked tab; the broadcast carries it so peers converge the
   *  lock too. Optional for forward-compat with daemons that pre-date the
   *  `tab_titles.locked` column. */
  locked?: boolean
}

/** WORKSPACE-SCOPED — a chat's Thread address changed because it was
 *  renamed (prd-thread-survives-tab-rename S3). The old address keeps
 *  working; views swap the address they send to and show. */
export interface SessionAddressChangedEvent {
  kind: 'session_address_changed'
  workspacePath: string
  paneGroupId: string | null
  conversationId: string
  address: string
  previousAddress: string
}

/** WORKSPACE-SCOPED — workspace tab-order/layout persistence advanced
 *  (#677). Carries the monotonic `revision` the daemon stamped on the
 *  write; a renderer re-fetches the layout when this revision is ahead of
 *  its last-known base (a remote client reordered) and drops a stale local
 *  write whose base is behind. `project`/`workspace` are the
 *  `(projectId, workspaceId)` key of the `workspace_layouts` row. */
export interface TabOrderChangedEvent {
  kind: 'tab_order_changed'
  workspacePath: string
  project: string
  workspace: string
  revision: number
}

/** WORKSPACE-SCOPED — a heartbeat session's live (PTY-attached) state
 *  flipped (#677). The daemon owns the PTY lifecycle and emits this when a
 *  heartbeat's `active_terminal_id` is stamped (live=true) or nulled
 *  (live=false). `project` is the projectId; `agent` is the heartbeat name.
 *  `workspacePath` is the project path the `?path=` filter matches.
 *  The event has no session id — settings refetches `heartbeat/list`. */
export interface HeartbeatStateChangedEvent {
  kind: 'heartbeat_state_changed'
  workspacePath: string
  project: string
  agent: string
  live: boolean
}

/** WORKSPACE-SCOPED — a project's heartbeat ROSTER mutated (a row was
 *  added / removed / archived / unarchived / enabled / disabled / edited /
 *  renamed / delivery re-targeted) — heartbeat-drawer live-update fix.
 *  Deliberately payload-free beyond addressing (the ProjectsChanged
 *  convention): consumers re-fetch the canonical list rather than patching
 *  from a diff, which stays honest for any mutation source (CLI, Settings,
 *  another window). Live (PTY) flips are NOT roster changes — those ride
 *  `heartbeat_state_changed`. `projectId` may be empty when the daemon
 *  couldn't resolve it at the emit site; consumers then rely on the
 *  socket's `?path=` filter alone. */
export interface HeartbeatRosterChangedEvent {
  kind: 'heartbeat_roster_changed'
  workspacePath: string
  projectId: string
}

/** APP-LEVEL — the registered project set changed (a project was added,
 *  removed, or re-registered) — 0.39.45, GH #18/#26. Payload-free by
 *  design: consumers re-fetch the canonical list (`fetchProjects`)
 *  rather than patching from a diff. Fires for mutations made by OTHER
 *  windows, the CLI (`k2so workspace create`), onboarding flows, and on
 *  remote daemons over K2 Connect — all the paths the old local-only
 *  Tauri `sync:projects` event never covered. */
export interface ProjectsChangedEvent {
  kind: 'projects_changed'
}

/** One aggregated PER-USER presence roster row — wire-frozen against
 *  `crates/k2-daemon/src/presence.rs::RosterUser` (S1, presence arc).
 *  `user` is `"owner"` for the synthesized owner row, else the username.
 *  There is no `viewer` role (prd-remove-viewer-role-v1.md). */
export interface PresenceRosterUser {
  user: string
  role: 'owner' | 'admin' | 'member'
  /** Open windows (app-level events sockets) this user holds. */
  windowCount: number
  /** Deduped, sorted workspace paths the user is viewing. */
  workspaces: string[]
  /** RETIRED — the daemon still sends it (always false) for older
   *  clients; nothing reads it. Edit grants went with the Viewer role. */
  grantedEdit?: boolean
  /** Unix seconds of the user's EARLIEST live connection. */
  connectedAt: number
}

/** APP-LEVEL — the presence roster changed (a connection registered or
 *  deregistered). Whole-set, last-write-wins — the ActiveChanged
 *  convention: replace the local roster with the carried one. The
 *  snapshot twin is `GET /cli/presence/roster` (fetched on hello). */
export interface PresenceChangedEvent {
  kind: 'presence_changed'
  roster: PresenceRosterUser[]
}

/** APP-LEVEL — the daemon asks the renderer to open a URL in K2's embedded
 *  browser tab (browser-pane arc, 0.40.34). Emitted when a session's shim
 *  (`k2 open <url>`) or a terminal hyperlink click routes an http(s) target
 *  through `/cli/fs/open-external`: when ≥1 session-events subscriber exists
 *  the daemon SKIPS its own `open`/`xdg-open` and emits this instead, so the
 *  URL surfaces INSIDE K2 (locally AND across a K2 Connect tunnel) rather than
 *  the host's system browser. Fires regardless of `?path=` (app-level). The
 *  Rust side has already validated the scheme is http/https — no scheme
 *  handling is needed here. */
export interface OpenUrlEvent {
  kind: 'open_url'
  url: string
  /** Where the open originated: the CLI shim vs a terminal hyperlink click. */
  source: 'shim' | 'terminal-link'
}

/** APP-LEVEL — something about the project-group set changed (remote
 *  live-update fix). Host-aware twin of the legacy loopback-only
 *  `project-group:*` Tauri events, which never arrive when connected to
 *  a REMOTE host over K2 Connect. `reason` is the legacy hook name minus
 *  its `project-group:` prefix: `groups-changed` | `members-changed` |
 *  `poc-changed` | `layout-changed` | `message-created` (typed as string
 *  for forward compat). Deliberately payload-lean — a refetch signal,
 *  consumers re-fetch canonical truth (the ProjectsChanged convention). */
export interface ProjectGroupsChangedEvent {
  kind: 'project_groups_changed'
  reason: string
}

/** APP-LEVEL — the feedback set changed (remote live-update fix). Twin
 *  of the legacy loopback-only `feedback:*` Tauri events. `reason` is
 *  the hook name minus its `feedback:` prefix: `created` | `answered` |
 *  `status-changed` | `commented` (string for forward compat). Refetch
 *  signal only — no item payload. */
export interface FeedbackChangedEvent {
  kind: 'feedback_changed'
  reason: string
}

/** APP-LEVEL — one ticket changed (prd-app-tickets-websocket-v1, 0.43.3).
 *  The app gateway's activity socket maps it to the room-scoped
 *  `ticket_changed` guest frame. This client ignores it: the Tickets board
 *  already refetches on `feedback_changed`. Ids and metadata only. */
export interface TicketChangedEvent {
  kind: 'ticket_changed'
  projectId: string
  id: string
  change: string
  status: string
  via: string | null
  hasBrief: boolean
}

/** 0.40.38 remote live-update — chat-history list mutated (session
 *  rename / pin / refresh). Payload-free refetch signal (ProjectsChanged
 *  convention). */
export interface ChatHistoryChangedEvent {
  kind: 'chat_history_changed'
}

/** APP-LEVEL — the token-usage ledger gained rows (0.40.150). Payload-free
 *  refetch signal (ProjectsChanged convention); the Settings → Token usage
 *  live log re-fetches its newest page and prepends new turns. */
export interface TokenUsageChangedEvent {
  kind: 'token_usage_changed'
}

/** APP-LEVEL — the LLM login wallet changed (add, sign-in state, switch,
 *  rename, remove, keep-warm refresh). Refetch signal for Settings → LLMs;
 *  `tool` names the tool when the daemon knows it. */
export interface LlmAccountsChangedEvent {
  kind: 'llm_accounts_changed'
  tool?: string | null
}

/** APP-LEVEL — files under a workspace changed (multi-writer Files-drawer
 *  live refresh). Agent shell writes on the daemon machine, other clients'
 *  `/cli/fs/*` mutations, and compress/upload completions all land here so
 *  every thin client (local multi-window + K2 Connect remote) can refresh
 *  its FileTree. `paths` are absolute; consumers refresh parent dirs
 *  present in their tree cache. Payload is a nudge + paths, not a full
 *  tree. Filter against the tree's own `rootPath` (APP-LEVEL so the
 *  always-live empty-`?path=` app socket delivers it). */
export interface FsChangedEvent {
  kind: 'fs_changed'
  workspacePath: string
  paths: string[]
}

/** APP-LEVEL — this computer's Zen files changed (prd-zen-mode-v1 Z12).
 *  Payload-free: the Zen store re-reads `GET /cli/zen/get` from the LOCAL
 *  daemon. Nothing about the file rides the frame. */
export interface ZenChangedEvent {
  kind: 'zen_changed'
}

/** APP-LEVEL — the daemon's hook installer could not update an agent CLI
 *  config (it did not parse, so it was left untouched) or hit a write
 *  error (prd-daemon-activity-and-thread-working-v1 A11). At most once per
 *  daemon boot. Replaces the Tauri-only `hook-injection-failed` event. */
export interface HooksInstallFailedEvent {
  kind: 'hooks_install_failed'
  failures: Array<{ cli: string; error: string; path?: string }>
}

/** prd-daemon-activity-and-thread-working-v1 DA19/§7.2: what one live
 *  session's agent is doing, as the daemon decided it. */
export type ActivityDisplay = 'working' | 'monitoring' | 'waiting' | 'idle' | 'unverifiable'

/** §7.2 — one activity row (one live v2 session). No tool or task names. */
/** A row's `counts` (0.45.2): live subagents, this turn's lead tool calls,
 *  and the shell commands among them. Reset to 0 when a turn starts. */
export interface ActivityRowCounts {
  subagents: number
  tools: number
  commands: number
}

export interface ActivityRow {
  sessionId: string
  agentName: string
  projectId: string | null
  workspacePath: string | null
  harness: string
  display: ActivityDisplay
  lead: {
    state: 'idle' | 'working' | 'waiting'
    outcome: 'none' | 'success' | 'failure' | 'cancelled' | 'boundary'
    since: number
    promptId: string | null
  }
  children: {
    subagents: number
    shells: number
    monitors: number
    crons: number
    unknown: number
    owed: number
    waiting: number
  }
  /** 0.45.2+: live subagents and this turn's lead tool calls / shell
   *  commands (numbers only). Absent from older servers and legacy rows. */
  counts?: ActivityRowCounts
  turnStartedAt: number | null
  evidenceAt: number | null
  evidenceSource: 'hook' | 'transcript' | 'title' | 'process' | 'screen' | null
  /** DA31 vocabulary (`k2-core/src/activity/fixtures/reasons.json`). */
  reason: string
  staleSince: number | null
  confirmed: boolean
  rev: number
}

/** RL1 — one workspace's rollup: the highest display across its sessions
 *  (`waiting > working > monitoring > unverifiable > idle`). */
export interface ActivityWorkspace {
  projectId: string | null
  workspacePath: string | null
  display: ActivityDisplay
  counts: Record<ActivityDisplay, number>
  /** The earliest `turnStartedAt` among the `working` sessions. */
  since: number | null
}

/** APP-LEVEL (RL2, §7.3) — one activity row changed or went away. `seq` is
 *  store-wide and gap-free; a gap, a new `instance_id` on hello, or a
 *  (re)opened socket means "pull `GET /cli/activity/snapshot`" (RL4). */
export interface ActivityChangedEvent {
  kind: 'activity_changed'
  seq: number
  instanceId: string
  row: ActivityRow | null
  removed: string | null
  turnEnded: { outcome: ActivityRow['lead']['outcome']; reason: string; at: number } | null
  workspace: ActivityWorkspace | null
}

/** §7.4 — `GET /cli/activity/snapshot[?workspace=]`. */
export interface ActivitySnapshot {
  instanceId: string
  seq: number
  serverNow: number
  staleAfterSecs: number
  rows: ActivityRow[]
  workspaces: ActivityWorkspace[]
}

export type SessionEventMessage =
  | SessionAddedEvent
  | SessionRemovedEvent
  | SessionRenamedEvent
  | HelloEvent
  | ActiveChangedEvent
  | LlmStatusChangedEvent
  | AgentStatusChangedEvent
  | SessionActivityChangedEvent
  | TunnelStatusChangedEvent
  | TunnelSubdomainsChangedEvent
  | PublishServicesChangedEvent
  | WorkspaceResourcesChangedEvent
  | TabTitleChangedEvent
  | SessionAddressChangedEvent
  | TabOrderChangedEvent
  | HeartbeatStateChangedEvent
  | HeartbeatRosterChangedEvent
  | ProjectsChangedEvent
  | PresenceChangedEvent
  | OpenUrlEvent
  | ProjectGroupsChangedEvent
  | FeedbackChangedEvent
  | TicketChangedEvent
  | ChatHistoryChangedEvent
  | TokenUsageChangedEvent
  | LlmAccountsChangedEvent
  | FsChangedEvent
  | ZenChangedEvent
  | HooksInstallFailedEvent
  | ActivityChangedEvent
  | ReviewQueueChangedEvent
  | ReviewChangedEvent
  | MailChangedEvent
  | DomainsChangedEvent
  | RemoteSessionAccessDeniedEvent

export interface SessionEventHandlers {
  onAdded?: (event: SessionAddedEvent) => void
  onRemoved?: (event: SessionRemovedEvent) => void
  onRenamed?: (event: SessionRenamedEvent) => void
  /** Fires after each successful (re)connect. Initial connect counts.
   *  Use it to trigger a one-shot reconcile so any events the renderer
   *  missed during the drop window get backfilled. */
  onHello?: (event: HelloEvent) => void
  /** Home M4 (MS46): a pinned room's ONE workspace socket also carries the
   *  tab-title / tab-order broadcasts (no second tab-events socket). */
  onTabTitleChanged?: (event: TabTitleChangedEvent) => void
  onTabOrderChanged?: (event: TabOrderChangedEvent) => void
  /** Home M4 (MS14/MS16): a pinned room's workspace socket is also its
   *  server's app-bus CARRIER while no app socket is open for that server:
   *  presence, the Active set, chat-history and file-change frames reach
   *  `onPresenceChanged(scope, …)` etc. One carrier per server dispatches
   *  (the first registered); the others stand by. */
  carryAppBus?: boolean
}

export type UnsubscribeFn = () => void

// ── Backoff config ───────────────────────────────────────────────────────

const INITIAL_BACKOFF_MS = 500
const MAX_BACKOFF_MS = 5_000

// ── Remote-recovery hold (0.40.48) ──────────────────────────────────────
//
// While the ACTIVE remote host is in a non-'connected' recovery state
// (ConnectionGate's health poll owns that verdict — down / booting /
// re-authing / wedged pool), a timed WS reconnect attempt CANNOT succeed;
// it just adds to the retry storm that hammered the tunnel edge in the
// 0.40.48 incident (three reconnect loops × un-jittered backoff). Instead
// of a timer, the factories below park on a store subscription and
// reconnect IMMEDIATELY when recovery lands back on 'connected' (the gate
// flips it after a confirmed-healthy /boot-status + session probe, so the
// daemon is genuinely ready for the WS upgrade). Local host: never blocked
// — `recovery` is meaningless while activeHost === 'local'.

/** Is `scope` the window's active host, and a remote that ConnectionGate
 *  says is recovering? A pinned scope for another server is never held by
 *  the window's recovery (its own recovery state is M2's pool). */
function remoteRecoveryBlocked(scope: ServerScope): boolean {
  if (!scope.isWindowHost()) return false
  const s = useConnectHostStore.getState()
  return s.activeHost !== 'local' && s.recovery.kind !== 'connected'
}

/** The window-level WS-drop debounce (remote-ws-drop) is about the
 *  window's connection; only sockets on the window's host feed it. */
function noteClosed(scope: ServerScope): void {
  if (scope.isWindowHost()) noteRemoteEventsClosed()
}

function noteOpened(scope: ServerScope): void {
  if (scope.isWindowHost()) noteRemoteEventsOpened()
}

/**
 * Fire `onRecovered` ONCE, as soon as the active host is no longer a
 * recovering remote (push-style — a store subscription, nothing polls or
 * spins). Also re-checks synchronously to cover the race where recovery
 * completed between the caller's `remoteRecoveryBlocked()` check and this
 * subscription (setRecovery dedupes same-kind writes, so a missed flip
 * would otherwise never re-notify). Returns a cancel fn.
 *
 * Exported (0.40.48): the tabs store's layout-save durability re-arm parks
 * on the same signal, so a split/reorder saved during a recovery window
 * flushes the moment the host is back instead of being silently dropped.
 */
export function onceRecovered(scope: ServerScope, onRecovered: () => void): () => void {
  let done = false
  const fire = (): void => {
    if (done) return
    done = true
    unsub()
    onRecovered()
  }
  const check = (s: ReturnType<typeof useConnectHostStore.getState>): void => {
    if (!scope.isWindowHost() || s.activeHost === 'local' || s.recovery.kind === 'connected') fire()
  }
  const unsub = useConnectHostStore.subscribe(check)
  check(useConnectHostStore.getState())
  return () => {
    done = true
    unsub()
  }
}

// ── Dispatch tables (derived from the kind registry) ─────────────────────

/** What `subscribeToWorkspaceSessionEvents` does per kind it owns. */
const WORKSPACE_SOCKET_DISPATCH: DispatchTable<'workspace', SessionEventHandlers> = {
  hello: (h, m) => h.onHello?.(m),
  session_added: (h, m) => h.onAdded?.(m),
  session_removed: (h, m) => h.onRemoved?.(m),
  session_renamed: (h, m) => h.onRenamed?.(m),
  tab_title_changed: (h, m) => h.onTabTitleChanged?.(m),
  tab_order_changed: (h, m) => h.onTabOrderChanged?.(m),
  // Every workspace socket feeds the Thread address bus; the sidecar
  // Thread views listen there (no socket of their own).
  session_address_changed: (_h, m) =>
    publishThreadAddress({
      address: m.address,
      previous: m.previousAddress,
      conversationId: m.conversationId,
      paneGroupId: m.paneGroupId,
      workspacePath: m.workspacePath,
    }),
}

// ── Public API ───────────────────────────────────────────────────────────

/** Subscribe to daemon session lifecycle events for one workspace.
 *
 *  Returns an unsubscribe function. Calling it tears down the WS and
 *  stops the reconnect loop. Safe to call multiple times (idempotent).
 *
 *  The handler callbacks are invoked synchronously inside the WS
 *  `onmessage`/`onopen` handlers — keep them cheap, or marshal off to
 *  a setTimeout if they trigger heavy state churn. */
export function subscribeToWorkspaceSessionEvents(
  scope: ServerScope,
  projectPath: string,
  handlers: SessionEventHandlers,
): UnsubscribeFn {
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  // 0.40.48: cancel fn for a pending recovery-hold (see onceRecovered).
  let recoveryWait: (() => void) | null = null
  let backoffMs = INITIAL_BACKOFF_MS
  let stopped = false
  const carrier = handlers.carryAppBus ? registerAppBusCarrier(scope) : null

  const clearReconnect = (): void => {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (recoveryWait !== null) {
      recoveryWait()
      recoveryWait = null
    }
  }

  const scheduleReconnect = (): void => {
    if (stopped) return
    clearReconnect()
    // 0.40.48: a recovering remote can't accept the upgrade — park on the
    // recovery state instead of burning timed attempts, and reconnect
    // immediately (fresh backoff) the moment the gate says 'connected'.
    if (remoteRecoveryBlocked(scope)) {
      recoveryWait = onceRecovered(scope, () => {
        recoveryWait = null
        if (stopped) return
        backoffMs = INITIAL_BACKOFF_MS
        void openSocket()
      })
      return
    }
    // Jitter (0.40.48): decorrelate the reconnect loops so parallel
    // subscribers don't retry in lockstep after a shared drop.
    const delay = jittered(backoffMs)
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void openSocket()
    }, delay)
  }

  /**
   * Issue #5: idempotent reconnect trigger fired from BOTH `onerror`
   * AND `onclose`. The WHATWG spec sequences `onerror → onclose` for
   * an aborted connection, and the pre-0.39.8 code relied on that —
   * `onerror` was a no-op trusting `onclose` would follow. In
   * practice WebKit under process throttling (App Nap), network-
   * stack pressure, or WebKit-Networking-side hiccups can fire
   * `onerror` WITHOUT a follow-up `onclose`, which left the
   * subscriber permanently dead (no reconnect ever scheduled).
   *
   * Calling `triggerReconnect()` from both is safe because of the
   * `reconnectTimer !== null` early-out: the normal
   * `onerror → onclose` sequence schedules exactly one timer (the
   * second call is a no-op), and the pathological
   * `onerror-without-onclose` case still schedules one. The backoff
   * progression isn't double-advanced.
   */
  const triggerReconnect = (): void => {
    if (stopped) return
    // R1: non-deliberate drop (not stop/unsub) — arm module-level debounce
    // across all three event factories. Deliberate stop returns above.
    noteClosed(scope)
    // Idempotent: a pending timer OR a pending recovery-hold means a
    // reconnect is already on its way (the onerror→onclose double-fire).
    if (reconnectTimer !== null || recoveryWait !== null) return
    scheduleReconnect()
  }

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: DaemonWsAvailable
    try {
      creds = await getDaemonWs(scope)
    } catch (err) {
      // Daemon not reachable yet — invalidate the cached creds so the
      // retry pulls fresh values off disk, then schedule a backoff
      // attempt.
      invalidateDaemonWs()
      console.warn('[session-events] daemon credentials unavailable, retrying:', err)
      // Hard open failure — same surface as a mid-flight drop for R1.
      noteClosed(scope)
      scheduleReconnect()
      return
    }

    if (stopped) return

    // 0.40.68 / GH#57: tear down any prior socket BEFORE opening a new
    // one. Leaving a half-open CONNECTING/CLOSING WS alongside a new
    // dial caused attach/detach thrash and mid-handshake EOF on the
    // daemon E2E listener (client aborted the abandoned handshake).
    if (socket) {
      const prev = socket
      socket = null
      try {
        prev.onopen = null
        prev.onmessage = null
        prev.onerror = null
        prev.onclose = null
        prev.close(1000, 'reconnect')
      } catch {
        // ignore
      }
    }

    // Pragmatic WS auth: in-memory session token on the query (desktop +
    // web). Hosted-web HTTP uses cookie-only; browsers also send the
    // k2_session cookie on same-origin WS upgrades as a second factor.
    const url = `${daemonWsBase(creds)}/cli/sessions/events?path=${encodeURIComponent(projectPath)}&token=${encodeURIComponent(creds.token)}`
    let ws: WebSocket
    try {
      // MS70: every daemon socket dials through the per-server queue under
      // the window-wide handshake cap.
      ws = await openQueuedWebSocket(scope, url)
    } catch (err) {
      console.warn('[session-events] WS construction failed:', err)
      noteClosed(scope)
      scheduleReconnect()
      return
    }
    if (stopped) {
      try {
        ws.close(1000, 'stopped')
      } catch {
        // ignore
      }
      return
    }
    socket = ws

    ws.onopen = () => {
      // Reset backoff so a long-lived stable connection doesn't pay
      // the previous failure's penalty on its next disconnect.
      backoffMs = INITIAL_BACKOFF_MS
      // R1/D4b: cancel WS-drop debounce; soft-resync grids if recovery
      // stayed connected through the blip.
      noteOpened(scope)
    }

    ws.onmessage = (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (raw === null) return
      let msg: SessionEventMessage
      try {
        msg = JSON.parse(raw) as SessionEventMessage
      } catch (err) {
        console.warn('[session-events] failed to parse frame:', err, raw)
        return
      }
      if (carrier) carrier.frame(msg)
      if (isResyncFrame(msg)) return
      // Home 0.43.2 (Z24/Z25): the kinds this socket handles come from the
      // shared registry. Every other registry kind (app-level frames the
      // daemon fans to every `?path=`, heartbeat frames the tab-events
      // socket owns, ignored kinds) is dropped silently; a kind the
      // registry doesn't know warns once per page.
      dispatchFrom(WORKSPACE_SOCKET_DISPATCH, 'workspace', handlers, msg)
    }

    ws.onerror = () => {
      // 0.39.8 (Issue #5): always trigger reconnect on error. The
      // pre-0.39.8 assumption that `onclose` reliably follows
      // `onerror` doesn't hold under WebKit Networking throttling.
      // `triggerReconnect` is idempotent — if `onclose` does
      // follow, its trigger is a no-op (timer already pending).
      triggerReconnect()
    }

    ws.onclose = (ev) => {
      if (socket === ws) {
        socket = null
      }
      // MS71: the pool tells a kick (4001) from a network drop.
      notePoolSocketClose(scope.hostKey, ev.code)
      if (stopped) return
      // A clean close (code 1000) from the server with no reason is
      // ambiguous — could be daemon shutdown, could be route gone.
      // Keep retrying; the daemon comes back fast enough that the
      // user won't notice, and a routed-away path would 403 on the
      // next attempt and the renderer just keeps trying (cheap).
      console.debug(
        `[session-events] WS closed (code=${ev.code}, reason="${ev.reason ?? ''}") — scheduling reconnect`,
      )
      logRemotePath('events-close', {
        bus: 'session-events',
        code: ev.code,
        wasClean: ev.wasClean,
        reason: ev.reason || undefined,
      })
      triggerReconnect()
    }
  }

  void openSocket()

  return () => {
    stopped = true
    carrier?.release()
    clearReconnect()
    if (socket) {
      try {
        socket.close(1000, 'unsubscribe')
      } catch {
        // ignore
      }
      socket = null
    }
  }
}

// ── App-level Active-state mirror (#672) ──────────────────────────────────
//
// The daemon owns the canonical Active set and pushes `active_changed`
// deltas (the WHOLE set) on the SAME session-events bus. Unlike
// `subscribeToWorkspaceSessionEvents` (one WS per active workspace, cwd-
// filtered), this is ONE app-level WS opened at boot that mirrors the
// daemon's Active set 1:1 into `useActiveStore`:
//
//   - on (re)connect (`Hello`) → GET /cli/projects/active snapshot →
//     `setFromSnapshot` (corrects any drift after a transient drop), and
//   - on each `active_changed` frame → `applyActiveChanged` (full-set
//     replace, last-write-wins).
//
// It subscribes with an EMPTY workspace path so it isn't scoped to a
// single workspace; the daemon broadcasts `active_changed` to every
// subscriber regardless of the cwd filter (the event carries no cwd).
// Per-workspace session_added/removed frames that happen to arrive on
// this socket are ignored — the workspace subscriber owns those.
//
// Capability-gated: against a daemon WITHOUT `canonical-active` the
// snapshot route 404s and no deltas arrive, so this is a no-op mirror and
// the Active bar uses its local-derivation fallback. We still open the WS
// (cheap) so a host that gains the capability mid-session starts mirroring
// on its next reconnect snapshot.

/** Fetch the canonical Active snapshot and write it into useActiveStore.
 *  `useActiveStore` mirrors the WINDOW's server, so only the primary scope
 *  writes it; for any other scope this is a no-op until M3 gives rooms
 *  their own Active store. No-op when the server doesn't advertise
 *  `canonical-active` (route absent). */
export async function refreshActiveSnapshot(scope: ServerScope): Promise<void> {
  if (!scope.isPrimary) return
  if (!scope.serverSupports('canonical-active')) return
  try {
    const snap = await daemonCliGet<{ projectIds: string[]; activeWindowHours: number }>(scope,
      'projects/active',
    )
    useActiveStore.getState().setFromSnapshot({
      projectIds: Array.isArray(snap?.projectIds) ? snap.projectIds : [],
      activeWindowHours:
        typeof snap?.activeWindowHours === 'number' ? snap.activeWindowHours : 24,
    })
  } catch (err) {
    // Route absent (old daemon) or transient failure — leave the mirror
    // alone; the local fallback derivation covers display.
    console.debug('[active-state] snapshot fetch skipped:', err)
  }
}

// ── App-level broadcast registry (#675) ───────────────────────────────────
//
// The Wave B APP-LEVEL events (llm/agent/tunnel) ride the SAME empty-path
// app-level WS that `subscribeToActiveState` already opens at boot. Rather
// than have each consumer open its own WS, consumers register a callback
// here; the app-level WS onmessage dispatches to all registered callbacks.
// The `onAppHello` registry lets consumers re-snapshot their truth on every
// (re)connect — the daemon may have dropped frames during the gap.
//
// All registries are module-level singletons (one app-level WS exists), so
// they survive the host-switch teardown/re-open of the WS itself — that's
// fine: the helpers below just (de)register pure callbacks. On a host
// switch the consumers' own host-aware snapshot fetch (fired from
// `onAppHello`) re-establishes truth against the new host.

type LlmStatusHandler = (e: LlmStatusChangedEvent) => void
type AgentStatusHandler = (e: AgentStatusChangedEvent) => void
type TunnelStatusHandler = (e: TunnelStatusChangedEvent) => void
// URLs & Ports drawer — the nested-subdomain map (whole-map replace, the
// ActiveChanged convention). `UrlsPortsSection.tsx` is the consumer.
type TunnelSubdomainsHandler = (e: TunnelSubdomainsChangedEvent) => void
type PublishServicesHandler = (e: PublishServicesChangedEvent) => void
type WorkspaceResourcesHandler = (e: WorkspaceResourcesChangedEvent) => void
/** RL4/A28: receives the hello frame (its `instance_id` tells a restart). */
type AppHelloHandler = (hello: HelloEvent) => void
/** RL4/A30: a `{"kind":"resync"}` control frame — the server is about to
 *  close this socket for lagging; re-read whatever it feeds. */
type AppResyncHandler = () => void
// #688 — app-level session add/remove. The per-workspace
// `subscribeToWorkspaceSessionEvents` only sees its OWN cwd; the
// Active-bar "live session" dot needs liveness across EVERY workspace
// without a poll. `SessionAdded`/`SessionRemoved` carry the session cwd
// in `workspace_path`, and the daemon's `event_matches_workspace` forwards
// any absolute-cwd event to the empty-`?path=` app-level subscriber (the
// `cwd.starts_with("/")` branch), so they DO arrive here. We dispatch them
// to these registries so `stores/active-agents.ts` can keep
// `liveSessionCwds` fresh push-style (no return to the 2.5s poll).
type SessionAddedHandler = (e: SessionAddedEvent) => void
type SessionRemovedHandler = (e: SessionRemovedEvent) => void

type ProjectsChangedHandler = (e: ProjectsChangedEvent) => void

// Presence S2 — APP-LEVEL `presence_changed` (whole-set roster replace).
// Rides the same app-level WS; `stores/presence.ts` is the consumer.
type PresenceChangedHandler = (e: PresenceChangedEvent) => void
type ActiveChangedHandler = (e: ActiveChangedEvent) => void

// Browser-pane arc (0.40.34) — APP-LEVEL `open_url` (daemon-routed URL
// opens: `k2 open <url>` shim + terminal hyperlink clicks through
// /cli/fs/open-external). Rides the same app-level WS; `stores/tabs.ts`
// is the consumer (`initOpenUrlBrowserTabs`, wired once from App.tsx).
// The handler receives the unwrapped `(url, source)` pair rather than the
// event object — consumers never need the `kind` discriminant.
type OpenUrlHandler = (url: string, source: OpenUrlEvent['source']) => void

// Remote live-update fix — APP-LEVEL `project_groups_changed` /
// `feedback_changed` refetch signals (host-aware twins of the legacy
// loopback-only `project-group:*` / `feedback:*` Tauri events).
// `stores/project-groups.ts` / `stores/feedback.ts` are the consumers.
// The handler receives the unwrapped `reason` string (the onOpenUrl
// idiom) — consumers never need the `kind` discriminant.
type ProjectGroupsChangedHandler = (reason: string) => void
type FeedbackChangedHandler = (reason: string) => void
type ChatHistoryChangedHandler = () => void
type TokenUsageChangedHandler = () => void
type LlmAccountsChangedHandler = (tool: string | null) => void
type ZenChangedHandler = () => void
type HooksInstallFailedHandler = (e: HooksInstallFailedEvent) => void
type ActivityChangedHandler = (e: ActivityChangedEvent) => void
type FsChangedHandler = (e: FsChangedEvent) => void
// Home 0.43.2 (Q7) — Settings → Email's refetch signal (`reason` unwrapped,
// the onFeedbackChanged idiom).
type MailChangedHandler = (reason: string) => void
// DN12e — Settings → Domains' refetch signal.
type DomainsChangedHandler = (e: DomainsChangedEvent) => void

type SessionActivityHandler = (e: SessionActivityChangedEvent) => void

// ── Per-server app event buses (Home M1) ──────────────────────────────────
//
// Each server has its own bus: the handler sets below live in an `AppBus`
// object, kept in a registry keyed by the scope's `id`. The primary scope's
// bus (`primary`) is the one every existing subscriber (FileTree,
// ChatHistory, AgentChatPane, the stores) attaches to; like the old
// module-level sets it survives a server switch, because its socket is torn
// down and re-opened against the new host by `subscribeToActiveState`.
//
// A bus for another server (`openAppBus(scopeForHost(b))`) is created on first
// use with NO socket: registering a handler never dials. Its socket opens
// only when something calls `subscribeToActiveState(thatScope)` — M4's room
// does that; M1 never does.

interface AppBusHandlers {
  llmStatus: Set<LlmStatusHandler>
  projectsChanged: Set<ProjectsChangedHandler>
  sessionActivity: Set<SessionActivityHandler>
  agentStatus: Set<AgentStatusHandler>
  tunnelStatus: Set<TunnelStatusHandler>
  tunnelSubdomains: Set<TunnelSubdomainsHandler>
  publishServices: Set<PublishServicesHandler>
  workspaceResources: Set<WorkspaceResourcesHandler>
  appHello: Set<AppHelloHandler>
  appResync: Set<AppResyncHandler>
  sessionAdded: Set<SessionAddedHandler>
  sessionRemoved: Set<SessionRemovedHandler>
  presenceChanged: Set<PresenceChangedHandler>
  openUrl: Set<OpenUrlHandler>
  projectGroupsChanged: Set<ProjectGroupsChangedHandler>
  feedbackChanged: Set<FeedbackChangedHandler>
  chatHistoryChanged: Set<ChatHistoryChangedHandler>
  tokenUsageChanged: Set<TokenUsageChangedHandler>
  llmAccountsChanged: Set<LlmAccountsChangedHandler>
  fsChanged: Set<FsChangedHandler>
  mailChanged: Set<MailChangedHandler>
  domainsChanged: Set<DomainsChangedHandler>
  activeChanged: Set<ActiveChangedHandler>
  zenChanged: Set<ZenChangedHandler>
  hooksInstallFailed: Set<HooksInstallFailedHandler>
  activityChanged: Set<ActivityChangedHandler>
}

interface BusState {
  readonly scopeId: string
  openSockets: number
  readonly handlers: AppBusHandlers
}

function createBusState(scopeId: string): BusState {
  return {
    scopeId,
    openSockets: 0,
    handlers: {
      llmStatus: new Set(),
      projectsChanged: new Set(),
      sessionActivity: new Set(),
      agentStatus: new Set(),
      tunnelStatus: new Set(),
      tunnelSubdomains: new Set(),
      publishServices: new Set(),
      workspaceResources: new Set(),
      appHello: new Set(),
      appResync: new Set(),
      sessionAdded: new Set(),
      sessionRemoved: new Set(),
      presenceChanged: new Set(),
      openUrl: new Set(),
      projectGroupsChanged: new Set(),
      feedbackChanged: new Set(),
      chatHistoryChanged: new Set(),
      tokenUsageChanged: new Set(),
      llmAccountsChanged: new Set(),
      fsChanged: new Set(),
      mailChanged: new Set(),
      domainsChanged: new Set(),
      activeChanged: new Set(),
      zenChanged: new Set(),
      hooksInstallFailed: new Set(),
      activityChanged: new Set(),
    },
  }
}

const _buses = new Map<string, BusState>()

function busFor(scope: ServerScope): BusState {
  let bus = _buses.get(scope.id)
  if (!bus) {
    bus = createBusState(scope.id)
    _buses.set(scope.id, bus)
  }
  return bus
}

function addHandler<T>(set: Set<T>, fn: T): UnsubscribeFn {
  set.add(fn)
  return () => void set.delete(fn)
}

/** Subscribe to APP-LEVEL `projects_changed` (0.39.45, GH #18/#26).
 *  Returns an unsubscribe fn. */
export function onProjectsChanged(scope: ServerScope, fn: ProjectsChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.projectsChanged, fn)
}

/** Subscribe to APP-LEVEL `llm_status_changed`. Returns an unsubscribe fn. */
export function onLlmStatusChanged(scope: ServerScope, fn: LlmStatusHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.llmStatus, fn)
}

/** Subscribe to APP-LEVEL `session_activity_changed` (0.40.39 daemon-side
 *  activity). Returns an unsubscribe fn. */
export function onSessionActivityChanged(
  scope: ServerScope,
  fn: SessionActivityHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.sessionActivity, fn)
}

/** Subscribe to APP-LEVEL `agent_status_changed`. Returns an unsubscribe fn. */
export function onAgentStatusChanged(scope: ServerScope, fn: AgentStatusHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.agentStatus, fn)
}

/** Subscribe to APP-LEVEL `tunnel_status_changed`. Returns an unsubscribe fn. */
export function onTunnelStatusChanged(scope: ServerScope, fn: TunnelStatusHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.tunnelStatus, fn)
}

/** Subscribe to APP-LEVEL `tunnel_subdomains_changed` (URLs & Ports
 *  drawer — the tunnel's nested-subdomain routing map). The event carries
 *  the whole map; replace, don't patch. Returns an unsubscribe fn. */
export function onTunnelSubdomainsChanged(
  scope: ServerScope,
  fn: TunnelSubdomainsHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.tunnelSubdomains, fn)
}

/** Subscribe to APP-LEVEL `publish_services_changed` (Published drawer —
 *  daemon-owned hosted services for a workspace). Carries `projectId`;
 *  consumers refetch `GET /cli/publish/list` when it matches. Returns an
 *  unsubscribe fn. */
export function onPublishServicesChanged(
  scope: ServerScope,
  fn: PublishServicesHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.publishServices, fn)
}

/** Subscribe to APP-LEVEL `workspace_resources_changed` (Files drawer +
 *  Projects Resources). Carries `workspaceId` (`projects.id`); consumers
 *  refetch this list when it matches. Returns an unsubscribe fn. */
export function onWorkspaceResourcesChanged(
  scope: ServerScope,
  fn: WorkspaceResourcesHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.workspaceResources, fn)
}

/** Fires on every app-level WS (re)connect — use it to re-snapshot truth
 *  that may have drifted while the socket was down. Returns an unsub fn. */
export function onAppHello(scope: ServerScope, fn: AppHelloHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.appHello, fn)
}

/** prd-daemon-activity-and-thread-working-v1 RL4/A30 — fires on an app
 *  socket's `{"kind":"resync"}` control frame (sent before a lag close), so
 *  consumers re-pull their snapshot. Returns an unsub fn. */
export function onAppResync(scope: ServerScope, fn: AppResyncHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.appResync, fn)
}

/** A `{"kind":"resync"}` control frame (RL4/A30). Not a bus event: no
 *  registry entry, handled before the dispatch table like `hello`. */
function isResyncFrame(msg: { kind: string }): boolean {
  return msg.kind === 'resync'
}

/** #688 — subscribe to APP-LEVEL `session_added` (EVERY workspace, not just
 *  the active one). The Active-bar live-session dot uses this to add the new
 *  session's cwd to `liveSessionCwds` the instant a PTY is registered (e.g.
 *  a pinned chat opened after startup), without the retired 2.5s poll.
 *  Returns an unsubscribe fn. */
export function onSessionAddedApp(scope: ServerScope, fn: SessionAddedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.sessionAdded, fn)
}

/** #688 — subscribe to APP-LEVEL `session_removed` (EVERY workspace).
 *  Returns an unsubscribe fn. */
export function onSessionRemovedApp(scope: ServerScope, fn: SessionRemovedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.sessionRemoved, fn)
}

/** Presence S2 — subscribe to APP-LEVEL `presence_changed` (the whole-set
 *  roster broadcast). Returns an unsubscribe fn. */
export function onPresenceChanged(scope: ServerScope, fn: PresenceChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.presenceChanged, fn)
}

/** Browser-pane arc (0.40.34) — subscribe to APP-LEVEL `open_url` (the
 *  daemon asks the renderer to surface a URL in K2's embedded browser
 *  tab). The callback receives `(url, source)`. Returns an unsubscribe
 *  fn. The primary bus survives the app-level WS teardown/reopen on a host
 *  switch, so one registration covers the app lifetime. */
export function onOpenUrl(scope: ServerScope, fn: OpenUrlHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.openUrl, fn)
}

/** Remote live-update fix — subscribe to APP-LEVEL `project_groups_changed`
 *  (the project-group set changed: structure / members / PoC / layout /
 *  chat message). The callback receives the unwrapped `reason` (the legacy
 *  hook name minus its `project-group:` prefix). Returns an unsubscribe
 *  fn. The primary bus survives the app-level WS teardown/reopen on a host
 *  switch, so one registration covers the app lifetime. */
export function onProjectGroupsChanged(
  scope: ServerScope,
  fn: ProjectGroupsChangedHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.projectGroupsChanged, fn)
}

/** Remote live-update fix — subscribe to APP-LEVEL `feedback_changed`
 *  (a feedback item was created / answered / status-changed / commented).
 *  The callback receives the unwrapped `reason` (the legacy hook name
 *  minus its `feedback:` prefix). Returns an unsubscribe fn. The primary
 *  bus survives host-switch WS teardown/reopen. */
export function onFeedbackChanged(scope: ServerScope, fn: FeedbackChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.feedbackChanged, fn)
}

/** 0.40.38 remote live-update — subscribe to APP-LEVEL
 *  `chat_history_changed` (chat session renamed / pinned / refreshed on
 *  the host). Refetch signal; the primary bus survives host-switch WS
 *  teardown/reopen. */
export function onChatHistoryChanged(
  scope: ServerScope,
  fn: ChatHistoryChangedHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.chatHistoryChanged, fn)
}

/** 0.40.150 — subscribe to APP-LEVEL `token_usage_changed` (the ledger
 *  scanner wrote new turns). Payload-free refetch signal; the primary bus
 *  survives host-switch WS teardown/reopen. Returns an unsub fn. */
export function onTokenUsageChanged(
  scope: ServerScope,
  fn: TokenUsageChangedHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.tokenUsageChanged, fn)
}

/** Subscribe to APP-LEVEL `llm_accounts_changed` (the LLM login wallet
 *  changed on that server). Returns an unsub fn. */
export function onLlmAccountsChanged(
  scope: ServerScope,
  fn: LlmAccountsChangedHandler,
): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.llmAccountsChanged, fn)
}

/** prd-zen-mode-v1 Z12 — subscribe to APP-LEVEL `zen_changed` (this
 *  computer's `~/.k2/zen/` re-validated to a new resolved page). Payload-free
 *  refetch signal. Zen listens on the LOCAL daemon's bus only
 *  (`lib/zen/zen-api.ts`). */
export function onZenChanged(scope: ServerScope, fn: ZenChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.zenChanged, fn)
}

/** A11 — subscribe to APP-LEVEL `hooks_install_failed` (the daemon could
 *  not install K2's hooks into an agent CLI config). */
export function onHooksInstallFailed(scope: ServerScope, fn: HooksInstallFailedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.hooksInstallFailed, fn)
}

/** RL2 — subscribe to APP-LEVEL `activity_changed` (the daemon's activity
 *  rows, prd-daemon-activity-and-thread-working-v1). */
export function onActivityChanged(scope: ServerScope, fn: ActivityChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.activityChanged, fn)
}

/** Files-drawer multi-writer live refresh — subscribe to APP-LEVEL
 *  `fs_changed` (paths under a workspace mutated on the host by agents,
 *  other clients, or `/cli/fs/*`). The primary bus survives host-switch
 *  WS teardown/reopen. FileTree filters `paths` / `workspacePath` against
 *  its own `rootPath`. */
export function onFsChanged(scope: ServerScope, fn: FsChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.fsChanged, fn)
}

/** Home 0.43.2 (Q7) — subscribe to APP-LEVEL `mail_changed` (mail server,
 *  domain or approval queue changed on `scope`'s server). Refetch signal;
 *  the handler gets the `reason`. Settings → Email is the consumer, so a
 *  remote Email page refreshes live (before this nothing listened). */
export function onMailChanged(scope: ServerScope, fn: MailChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.mailChanged, fn)
}

/** prd-dns-pending-and-cutover-safety-v1 DN12e — subscribe to APP-LEVEL
 *  `domains_changed` on `scope`'s server (custom domains attached, removed,
 *  re-checked, or flipped by the daemon's own pending-zone re-check).
 *  Refetch signal; Settings → K2 Server → Domains is the consumer. */
export function onDomainsChanged(scope: ServerScope, fn: DomainsChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.domainsChanged, fn)
}

/** Home M4: the server's whole Active set changed (`active_changed`).
 *  Fed by a pinned room's carrier socket; the window's own Active set
 *  stays on `subscribeToActiveState` → `useActiveStore`. */
export function onActiveChanged(scope: ServerScope, fn: ActiveChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.activeChanged, fn)
}

/** Live handler count on `scope`'s bus (tests and leak checks, MS52 j). */
export function appBusHandlerCount(scope: ServerScope): number {
  let n = 0
  for (const set of Object.values(busFor(scope).handlers) as Array<Set<unknown>>) n += set.size
  return n
}

// ── App-bus carriers (Home M4) ────────────────────────────────────────────
//
// A server with an open pinned room but no app socket of its own (every
// server but the window's) gets its app-level frames from the room's
// workspace socket: the daemon forwards every app-level event to every
// subscriber regardless of `?path=`. Only the HEAD carrier per server
// dispatches, so two rooms on one server never deliver a frame twice; when
// it closes, the next one takes over. A dedicated app socket
// (`subscribeToActiveState`) on that server always wins.

// App-level frames a carrier forwards: `CARRIED_KINDS`, derived from the
// kind registry (`session-event-kinds.ts`, `carried: true`). Not `open_url`
// (a view-only room never opens tabs on B's say-so) and not session
// add/remove (the room's own workspace handlers own those). Home 0.43.2
// (Z22): `agent_status_changed` is carried, so a room sees hook status.

const _carriers = new Map<string, Array<symbol>>()

function registerAppBusCarrier(scope: ServerScope): { frame(msg: SessionEventMessage): void; release(): void } {
  const token = Symbol(scope.id)
  const list = _carriers.get(scope.id) ?? []
  list.push(token)
  _carriers.set(scope.id, list)
  const isHead = (): boolean => _carriers.get(scope.id)?.[0] === token
  return {
    frame(msg) {
      if (!isHead()) return
      const bus = busFor(scope)
      if (bus.openSockets > 0) return
      if (msg.kind === 'hello') {
        for (const fn of bus.handlers.appHello) fn(msg)
        return
      }
      if (isResyncFrame(msg)) {
        for (const fn of bus.handlers.appResync) fn()
        return
      }
      if (!CARRIED_KINDS.has(msg.kind)) return
      dispatchAppEvent(bus, msg)
    },
    release() {
      const cur = _carriers.get(scope.id)
      if (!cur) return
      const next = cur.filter((t) => t !== token)
      if (next.length === 0) _carriers.delete(scope.id)
      else _carriers.set(scope.id, next)
    },
  }
}

/** Test seam: carriers registered for `scope`'s server. */
export function appBusCarrierCountForTests(scope: ServerScope): number {
  return _carriers.get(scope.id)?.length ?? 0
}

/** What the app socket (and a carrier) does per kind the registry marks
 *  `app: true`. A kind that gains `app` without an entry here does not
 *  compile. `hello` is handled before the table by both callers (it
 *  re-snapshots); its entry fans out to `onAppHello`. */
const APP_SOCKET_DISPATCH: DispatchTable<'app', AppBusHandlers> = {
  hello: (h, m) => {
    for (const fn of h.appHello) fn(m)
  },
  active_changed: (h, m) => {
    for (const fn of h.activeChanged) fn(m)
  },
  llm_status_changed: (h, m) => {
    for (const fn of h.llmStatus) fn(m)
  },
  projects_changed: (h, m) => {
    for (const fn of h.projectsChanged) fn(m)
  },
  session_activity_changed: (h, m) => {
    for (const fn of h.sessionActivity) fn(m)
  },
  agent_status_changed: (h, m) => {
    for (const fn of h.agentStatus) fn(m)
  },
  tunnel_status_changed: (h, m) => {
    for (const fn of h.tunnelStatus) fn(m)
  },
  tunnel_subdomains_changed: (h, m) => {
    for (const fn of h.tunnelSubdomains) fn(m)
  },
  publish_services_changed: (h, m) => {
    for (const fn of h.publishServices) fn(m)
  },
  workspace_resources_changed: (h, m) => {
    for (const fn of h.workspaceResources) fn(m)
  },
  session_added: (h, m) => {
    for (const fn of h.sessionAdded) fn(m)
  },
  session_removed: (h, m) => {
    for (const fn of h.sessionRemoved) fn(m)
  },
  presence_changed: (h, m) => {
    for (const fn of h.presenceChanged) fn(m)
  },
  open_url: (h, m) => {
    for (const fn of h.openUrl) fn(m.url, m.source)
  },
  project_groups_changed: (h, m) => {
    for (const fn of h.projectGroupsChanged) fn(m.reason)
  },
  feedback_changed: (h, m) => {
    for (const fn of h.feedbackChanged) fn(m.reason)
  },
  chat_history_changed: (h) => {
    for (const fn of h.chatHistoryChanged) fn()
  },
  token_usage_changed: (h) => {
    for (const fn of h.tokenUsageChanged) fn()
  },
  llm_accounts_changed: (h, m) => {
    for (const fn of h.llmAccountsChanged) fn(m.tool ?? null)
  },
  fs_changed: (h, m) => {
    for (const fn of h.fsChanged) fn(m)
  },
  mail_changed: (h, m) => {
    for (const fn of h.mailChanged) fn(m.reason)
  },
  domains_changed: (h, m) => {
    for (const fn of h.domainsChanged) fn(m)
  },
  zen_changed: (h) => {
    for (const fn of h.zenChanged) fn()
  },
  hooks_install_failed: (h, m) => {
    for (const fn of h.hooksInstallFailed) fn(m)
  },
  activity_changed: (h, m) => {
    for (const fn of h.activityChanged) fn(m)
  },
}

/** Dispatch one app-level frame on `bus`. `socket` names the caller for
 *  the unknown-kind warning. */
function dispatchAppEvent(bus: BusState, msg: SessionEventMessage, socket = 'app'): void {
  dispatchFrom(APP_SOCKET_DISPATCH, socket, bus.handlers, msg)
}

/** One server's app-level event bus (MS16): the 18 `on*` subscriptions,
 *  bound to that server. Getting it opens nothing; the socket that feeds it
 *  is `subscribeToActiveState(scope)`. */
export interface AppBus {
  /** The owning scope's registry id (`primary` or `host:<hostKey>`). */
  readonly scopeId: string
  /** Live `subscribeToActiveState` sockets feeding this bus. */
  readonly openSockets: number
  /** Registered handler count, for tests and leak checks. */
  handlerCount(): number
  onProjectsChanged(fn: ProjectsChangedHandler): UnsubscribeFn
  onLlmStatusChanged(fn: LlmStatusHandler): UnsubscribeFn
  onSessionActivityChanged(fn: SessionActivityHandler): UnsubscribeFn
  onAgentStatusChanged(fn: AgentStatusHandler): UnsubscribeFn
  onTunnelStatusChanged(fn: TunnelStatusHandler): UnsubscribeFn
  onTunnelSubdomainsChanged(fn: TunnelSubdomainsHandler): UnsubscribeFn
  onPublishServicesChanged(fn: PublishServicesHandler): UnsubscribeFn
  onWorkspaceResourcesChanged(fn: WorkspaceResourcesHandler): UnsubscribeFn
  onAppHello(fn: AppHelloHandler): UnsubscribeFn
  onSessionAddedApp(fn: SessionAddedHandler): UnsubscribeFn
  onSessionRemovedApp(fn: SessionRemovedHandler): UnsubscribeFn
  onPresenceChanged(fn: PresenceChangedHandler): UnsubscribeFn
  onOpenUrl(fn: OpenUrlHandler): UnsubscribeFn
  onProjectGroupsChanged(fn: ProjectGroupsChangedHandler): UnsubscribeFn
  onFeedbackChanged(fn: FeedbackChangedHandler): UnsubscribeFn
  onChatHistoryChanged(fn: ChatHistoryChangedHandler): UnsubscribeFn
  onTokenUsageChanged(fn: TokenUsageChangedHandler): UnsubscribeFn
  onLlmAccountsChanged(fn: LlmAccountsChangedHandler): UnsubscribeFn
  onFsChanged(fn: FsChangedHandler): UnsubscribeFn
  onMailChanged(fn: MailChangedHandler): UnsubscribeFn
  onDomainsChanged(fn: DomainsChangedHandler): UnsubscribeFn
  onZenChanged(fn: ZenChangedHandler): UnsubscribeFn
  onHooksInstallFailed(fn: HooksInstallFailedHandler): UnsubscribeFn
  onActivityChanged(fn: ActivityChangedHandler): UnsubscribeFn
}

const _facades = new Map<string, AppBus>()

/** The app-level event bus for `scope`'s server. Creating it opens no
 *  socket. The same object comes back for the same scope id. */
export function openAppBus(scope: ServerScope): AppBus {
  let facade = _facades.get(scope.id)
  if (facade) return facade
  const state = busFor(scope)
  facade = {
    scopeId: state.scopeId,
    get openSockets(): number {
      return state.openSockets
    },
    handlerCount(): number {
      let n = 0
      for (const set of Object.values(state.handlers) as Array<Set<unknown>>) n += set.size
      return n
    },
    onProjectsChanged: (fn) => onProjectsChanged(scope, fn),
    onLlmStatusChanged: (fn) => onLlmStatusChanged(scope, fn),
    onSessionActivityChanged: (fn) => onSessionActivityChanged(scope, fn),
    onAgentStatusChanged: (fn) => onAgentStatusChanged(scope, fn),
    onTunnelStatusChanged: (fn) => onTunnelStatusChanged(scope, fn),
    onTunnelSubdomainsChanged: (fn) => onTunnelSubdomainsChanged(scope, fn),
    onPublishServicesChanged: (fn) => onPublishServicesChanged(scope, fn),
    onWorkspaceResourcesChanged: (fn) => onWorkspaceResourcesChanged(scope, fn),
    onAppHello: (fn) => onAppHello(scope, fn),
    onSessionAddedApp: (fn) => onSessionAddedApp(scope, fn),
    onSessionRemovedApp: (fn) => onSessionRemovedApp(scope, fn),
    onPresenceChanged: (fn) => onPresenceChanged(scope, fn),
    onOpenUrl: (fn) => onOpenUrl(scope, fn),
    onProjectGroupsChanged: (fn) => onProjectGroupsChanged(scope, fn),
    onFeedbackChanged: (fn) => onFeedbackChanged(scope, fn),
    onChatHistoryChanged: (fn) => onChatHistoryChanged(scope, fn),
    onTokenUsageChanged: (fn) => onTokenUsageChanged(scope, fn),
    onLlmAccountsChanged: (fn) => onLlmAccountsChanged(scope, fn),
    onFsChanged: (fn) => onFsChanged(scope, fn),
    onMailChanged: (fn) => onMailChanged(scope, fn),
    onDomainsChanged: (fn) => onDomainsChanged(scope, fn),
    onZenChanged: (fn) => onZenChanged(scope, fn),
    onHooksInstallFailed: (fn) => onHooksInstallFailed(scope, fn),
    onActivityChanged: (fn) => onActivityChanged(scope, fn),
  }
  _facades.set(scope.id, facade)
  return facade
}

/**
 * Open the single app-level Active-state subscription. Call once at app
 * boot. Returns an UnsubscribeFn that tears down the WS + reconnect loop.
 * On a host switch, tear this down and call it again (see connect-host
 * wiring) so a remote host's Active set mirrors 1:1.
 *
 * This socket is ALSO the carrier for the Wave B APP-LEVEL broadcasts
 * (llm/agent/tunnel, #675). They're dispatched to the registries above so
 * consumers (`stores/assistant.ts`, `stores/active-agents.ts`,
 * `CompanionSection.tsx`) can drop their polling loops. On (re)connect the
 * `Hello` frame also fans out to `onAppHello` so those consumers re-snapshot.
 */
export function subscribeToActiveState(scope: ServerScope): UnsubscribeFn {
  const bus = busFor(scope)
  bus.openSockets += 1
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  // 0.40.48: cancel fn for a pending recovery-hold (see onceRecovered).
  let recoveryWait: (() => void) | null = null
  let backoffMs = INITIAL_BACKOFF_MS
  let stopped = false

  const clearReconnect = (): void => {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (recoveryWait !== null) {
      recoveryWait()
      recoveryWait = null
    }
  }

  const scheduleReconnect = (): void => {
    if (stopped) return
    clearReconnect()
    // 0.40.48: a recovering remote can't accept the upgrade — park on the
    // recovery state instead of burning timed attempts, and reconnect
    // immediately (fresh backoff) the moment the gate says 'connected'.
    if (remoteRecoveryBlocked(scope)) {
      recoveryWait = onceRecovered(scope, () => {
        recoveryWait = null
        if (stopped) return
        backoffMs = INITIAL_BACKOFF_MS
        void openSocket()
      })
      return
    }
    // Jitter (0.40.48): decorrelate the reconnect loops so parallel
    // subscribers don't retry in lockstep after a shared drop.
    const delay = jittered(backoffMs)
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void openSocket()
    }, delay)
  }

  const triggerReconnect = (): void => {
    if (stopped) return
    // R1: non-deliberate drop — shared debounce across event factories.
    noteClosed(scope)
    // Idempotent: a pending timer OR a pending recovery-hold means a
    // reconnect is already on its way (the onerror→onclose double-fire).
    if (reconnectTimer !== null || recoveryWait !== null) return
    scheduleReconnect()
  }

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: DaemonWsAvailable
    try {
      creds = await getDaemonWs(scope)
    } catch (err) {
      invalidateDaemonWs()
      console.warn('[active-state] daemon credentials unavailable, retrying:', err)
      noteClosed(scope)
      scheduleReconnect()
      return
    }
    if (stopped) return

    // 0.40.68 / GH#57: close prior socket before redial (see session-events
    // workspace subscriber — same attach/detach thrash class).
    if (socket) {
      const prev = socket
      socket = null
      try {
        prev.onopen = null
        prev.onmessage = null
        prev.onerror = null
        prev.onclose = null
        prev.close(1000, 'reconnect')
      } catch {
        // ignore
      }
    }

    // Empty path → app-level subscriber (not scoped to one workspace).
    const url = `${daemonWsBase(creds)}/cli/sessions/events?path=&token=${encodeURIComponent(creds.token)}`
    let ws: WebSocket
    try {
      // MS70: every daemon socket dials through the per-server queue under
      // the window-wide handshake cap.
      ws = await openQueuedWebSocket(scope, url)
    } catch (err) {
      console.warn('[active-state] WS construction failed:', err)
      noteClosed(scope)
      scheduleReconnect()
      return
    }
    if (stopped) {
      try {
        ws.close(1000, 'stopped')
      } catch {
        // ignore
      }
      return
    }
    socket = ws

    ws.onopen = () => {
      backoffMs = INITIAL_BACKOFF_MS
      noteOpened(scope)
    }

    ws.onmessage = (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (raw === null) return
      let msg: SessionEventMessage
      try {
        msg = JSON.parse(raw) as SessionEventMessage
      } catch (err) {
        console.warn('[active-state] failed to parse frame:', err, raw)
        return
      }
      if (msg.kind === 'hello') {
        // (Re)connected — pull a fresh snapshot to correct any drift
        // (deltas may have been missed during a drop window).
        void refreshActiveSnapshot(scope)
        // Fan out to Wave B app-level consumers so they re-snapshot their
        // own truth (llm/agent/tunnel) after the same drop window.
        for (const h of bus.handlers.appHello) h(msg)
        return
      }
      if (isResyncFrame(msg)) {
        for (const h of bus.handlers.appResync) h()
        return
      }
      if (msg.kind === 'active_changed' && scope.isPrimary) {
        // useActiveStore mirrors the window's server only (see
        // refreshActiveSnapshot). The bus dispatch below still runs.
        useActiveStore.getState().applyActiveChanged({
          activeProjectIds: Array.isArray(msg.activeProjectIds) ? msg.activeProjectIds : [],
          activeWindowHours:
            typeof msg.activeWindowHours === 'number' ? msg.activeWindowHours : 24,
        })
      }
      // Every kind the registry marks `app` (Wave B llm/agent/tunnel, the
      // refetch signals, #688 session add/remove for the live-session dot,
      // mail) dispatches on this server's bus. Workspace-owned kinds
      // (session_renamed, tab_*, heartbeat_*) and ignored ones (review_*)
      // drop silently; a kind the registry doesn't know warns once.
      // Before 0.43.2 this was a hand-kept `if` chain that had already
      // missed `projects_changed` once.
      dispatchAppEvent(bus, msg)
    }

    ws.onerror = () => {
      triggerReconnect()
    }

    ws.onclose = (ev) => {
      if (socket === ws) socket = null
      // MS71: the pool tells a kick (4001) from a network drop.
      notePoolSocketClose(scope.hostKey, ev.code)
      if (stopped) return
      console.debug(
        `[active-state] WS closed (code=${ev.code}, reason="${ev.reason ?? ''}") — scheduling reconnect`,
      )
      logRemotePath('events-close', {
        bus: 'active-state',
        code: ev.code,
        wasClean: ev.wasClean,
        reason: ev.reason || undefined,
      })
      triggerReconnect()
    }
  }

  void openSocket()

  return () => {
    if (!stopped) bus.openSockets -= 1
    stopped = true
    clearReconnect()
    if (socket) {
      try {
        socket.close(1000, 'unsubscribe')
      } catch {
        // ignore
      }
      socket = null
    }
  }
}

// ── Workspace-scoped tab / heartbeat subscription (#676/#677) ──────────────
//
// The Wave B WORKSPACE-SCOPED tab-title / tab-order / heartbeat events
// (`tab_title_changed`, `tab_order_changed`, `heartbeat_state_changed`,
// `heartbeat_roster_changed`) are
// forwarded only to subscribers whose `?path=` matches the carried
// `workspacePath` (cwd-prefix rule). An empty `workspacePath` matches
// no workspace subscriber — `event_matches_workspace` does not
// special-case an empty cwd, so emitters pass the project path.
// This helper opens that per-workspace socket and invokes
// the caller's handlers; consumers (`stores/tabs.ts`,
// `stores/heartbeat-sessions.ts`) re-snapshot their truth on each
// (re)connect (`onHello`) to backfill anything missed during a drop.
// Tearing down + re-subscribing on a workspace switch is the caller's
// responsibility (the path is baked into the URL).

export interface WorkspaceTabHandlers {
  /** A tab's daemon-canonical title changed (rename in another client). */
  onTabTitleChanged?: (event: TabTitleChangedEvent) => void
  /** The workspace layout/tab-order persistence advanced (carries the
   *  monotonic revision for last-write-wins conflict resolution). */
  onTabOrderChanged?: (event: TabOrderChangedEvent) => void
  /** A heartbeat session's live (PTY-attached) state flipped. */
  onHeartbeatStateChanged?: (event: HeartbeatStateChangedEvent) => void
  /** The project's heartbeat roster mutated (CRUD from any source) —
   *  re-fetch the list; the event carries no row data by design. */
  onHeartbeatRosterChanged?: (event: HeartbeatRosterChangedEvent) => void
  /** Fires after each successful (re)connect — re-snapshot here. */
  onHello?: (event: HelloEvent) => void
}

/** What `subscribeToWorkspaceTabEvents` does per kind it owns (registry
 *  `tabs: true`). */
const TAB_SOCKET_DISPATCH: DispatchTable<'tabs', WorkspaceTabHandlers> = {
  hello: (h, m) => h.onHello?.(m),
  tab_title_changed: (h, m) => h.onTabTitleChanged?.(m),
  tab_order_changed: (h, m) => h.onTabOrderChanged?.(m),
  heartbeat_state_changed: (h, m) => h.onHeartbeatStateChanged?.(m),
  heartbeat_roster_changed: (h, m) => h.onHeartbeatRosterChanged?.(m),
}

/** Subscribe to WORKSPACE-SCOPED tab/heartbeat events for one workspace
 *  path. Returns an unsubscribe fn that tears down the WS + reconnect loop. */
export function subscribeToWorkspaceTabEvents(
  scope: ServerScope,
  workspacePath: string,
  handlers: WorkspaceTabHandlers,
): UnsubscribeFn {
  let socket: WebSocket | null = null
  let reconnectTimer: ReturnType<typeof setTimeout> | null = null
  // 0.40.48: cancel fn for a pending recovery-hold (see onceRecovered).
  let recoveryWait: (() => void) | null = null
  let backoffMs = INITIAL_BACKOFF_MS
  let stopped = false

  const clearReconnect = (): void => {
    if (reconnectTimer !== null) {
      clearTimeout(reconnectTimer)
      reconnectTimer = null
    }
    if (recoveryWait !== null) {
      recoveryWait()
      recoveryWait = null
    }
  }

  const scheduleReconnect = (): void => {
    if (stopped) return
    clearReconnect()
    // 0.40.48: a recovering remote can't accept the upgrade — park on the
    // recovery state instead of burning timed attempts, and reconnect
    // immediately (fresh backoff) the moment the gate says 'connected'.
    if (remoteRecoveryBlocked(scope)) {
      recoveryWait = onceRecovered(scope, () => {
        recoveryWait = null
        if (stopped) return
        backoffMs = INITIAL_BACKOFF_MS
        void openSocket()
      })
      return
    }
    // Jitter (0.40.48): decorrelate the reconnect loops so parallel
    // subscribers don't retry in lockstep after a shared drop.
    const delay = jittered(backoffMs)
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
    reconnectTimer = setTimeout(() => {
      reconnectTimer = null
      void openSocket()
    }, delay)
  }

  const triggerReconnect = (): void => {
    if (stopped) return
    // R1: non-deliberate drop — shared debounce across event factories.
    noteClosed(scope)
    // Idempotent: a pending timer OR a pending recovery-hold means a
    // reconnect is already on its way (the onerror→onclose double-fire).
    if (reconnectTimer !== null || recoveryWait !== null) return
    scheduleReconnect()
  }

  const openSocket = async (): Promise<void> => {
    if (stopped) return
    let creds: DaemonWsAvailable
    try {
      creds = await getDaemonWs(scope)
    } catch (err) {
      invalidateDaemonWs()
      console.warn('[tab-events] daemon credentials unavailable, retrying:', err)
      noteClosed(scope)
      scheduleReconnect()
      return
    }
    if (stopped) return

    // 0.40.68 / GH#57: close prior socket before redial (see session-events
    // workspace subscriber — same attach/detach thrash class).
    if (socket) {
      const prev = socket
      socket = null
      try {
        prev.onopen = null
        prev.onmessage = null
        prev.onerror = null
        prev.onclose = null
        prev.close(1000, 'reconnect')
      } catch {
        // ignore
      }
    }

    const url = `${daemonWsBase(creds)}/cli/sessions/events?path=${encodeURIComponent(workspacePath)}&token=${encodeURIComponent(creds.token)}`
    let ws: WebSocket
    try {
      // MS70: every daemon socket dials through the per-server queue under
      // the window-wide handshake cap.
      ws = await openQueuedWebSocket(scope, url)
    } catch (err) {
      console.warn('[tab-events] WS construction failed:', err)
      noteClosed(scope)
      scheduleReconnect()
      return
    }
    if (stopped) {
      try {
        ws.close(1000, 'stopped')
      } catch {
        // ignore
      }
      return
    }
    socket = ws

    ws.onopen = () => {
      backoffMs = INITIAL_BACKOFF_MS
      noteOpened(scope)
    }

    ws.onmessage = (ev) => {
      const raw = typeof ev.data === 'string' ? ev.data : null
      if (raw === null) return
      let msg: SessionEventMessage
      try {
        msg = JSON.parse(raw) as SessionEventMessage
      } catch (err) {
        console.warn('[tab-events] failed to parse frame:', err, raw)
        return
      }
      // session_added/removed/renamed, app-level and review frames arrive
      // here too; their own subscribers own them, so they drop silently.
      // A kind the registry doesn't know warns once (Z39).
      dispatchFrom(TAB_SOCKET_DISPATCH, 'tab-events', handlers, msg)
    }

    ws.onerror = () => {
      triggerReconnect()
    }

    ws.onclose = (ev) => {
      if (socket === ws) socket = null
      // MS71: the pool tells a kick (4001) from a network drop.
      notePoolSocketClose(scope.hostKey, ev.code)
      if (stopped) return
      console.debug(
        `[tab-events] WS closed (code=${ev.code}, reason="${ev.reason ?? ''}") — scheduling reconnect`,
      )
      logRemotePath('events-close', {
        bus: 'tab-events',
        code: ev.code,
        wasClean: ev.wasClean,
        reason: ev.reason || undefined,
      })
      triggerReconnect()
    }
  }

  void openSocket()

  return () => {
    stopped = true
    clearReconnect()
    if (socket) {
      try {
        socket.close(1000, 'unsubscribe')
      } catch {
        // ignore
      }
      socket = null
    }
  }
}
