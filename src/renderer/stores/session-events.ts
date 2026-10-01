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
   *  /boot-status carries the camelCase `instanceId`). Restart DETECTION
   *  is owned by ConnectionGate's boot-status health poll — consumers here
   *  already re-snapshot on every hello, which covers the restart case —
   *  so this is currently informational/wire-documenting only. */
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
 *  list-running + agent-status poll in `stores/active-agents.ts`. */
export interface AgentStatusChangedEvent {
  kind: 'agent_status_changed'
  /** The `K2SO_PANE_ID` (== terminal id) the PTY was spawned with. */
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
// ReviewPanel surfaces were deleted with the 0.40.31 cleanup. Unknown kinds
// fall through every subscriber's `default` arm, so no types are needed here.

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
  | TabOrderChangedEvent
  | HeartbeatStateChangedEvent
  | HeartbeatRosterChangedEvent
  | ProjectsChangedEvent
  | PresenceChangedEvent
  | OpenUrlEvent
  | ProjectGroupsChangedEvent
  | FeedbackChangedEvent
  | ChatHistoryChangedEvent
  | TokenUsageChangedEvent
  | FsChangedEvent

export interface SessionEventHandlers {
  onAdded?: (event: SessionAddedEvent) => void
  onRemoved?: (event: SessionRemovedEvent) => void
  onRenamed?: (event: SessionRenamedEvent) => void
  /** Fires after each successful (re)connect. Initial connect counts.
   *  Use it to trigger a one-shot reconcile so any events the renderer
   *  missed during the drop window get backfilled. */
  onHello?: (event: HelloEvent) => void
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
      switch (msg.kind) {
        case 'hello':
          handlers.onHello?.(msg)
          break
        case 'session_added':
          handlers.onAdded?.(msg)
          break
        case 'session_removed':
          handlers.onRemoved?.(msg)
          break
        case 'session_renamed':
          handlers.onRenamed?.(msg)
          break
        case 'active_changed':
        case 'presence_changed':
        case 'open_url':
        case 'projects_changed':
        case 'project_groups_changed':
        case 'feedback_changed':
        case 'chat_history_changed':
        case 'token_usage_changed':
        case 'session_activity_changed':
        case 'publish_services_changed':
        case 'workspace_resources_changed':
          // App-level concerns (#672 / presence S2 / browser-pane 0.40.34
          // / remote live-update fix / published services / workspace
          // resources) — the per-workspace subscriber ignores them;
          // `subscribeToActiveState` consumes them. Swallow here so they
          // don't hit the unknown-kind warning (the daemon forwards every
          // app-level event to every subscriber regardless of `?path=`).
          break
        case 'tab_title_changed':
        case 'tab_order_changed':
        case 'heartbeat_state_changed':
        case 'heartbeat_roster_changed':
          // 0.39.39 (#676/#677) — these workspace-scoped broadcasts share the
          // `/cli/sessions/events` channel but are OWNED by
          // `subscribeToWorkspaceTabEvents` (it adopts the reorder / applies the
          // title). This session-events subscriber doesn't handle them; swallow
          // here so it doesn't warn "unknown event kind" on every broadcast
          // frame (the daemon emits `tab_order_changed` ~4×/sec during a remote
          // reorder). No double-adoption: only the tab-events subscriber acts.
          break
        case 'fs_changed':
          // APP-LEVEL multi-writer FS refresh — owned by the app-level
          // Active-state socket (`subscribeToActiveState` → onFsChanged).
          // Daemon still fans app-level frames to per-workspace sockets;
          // swallow here so remote hosts don't spam "unknown event kind".
          break
        default: {
          // Unknown kind — forward-compat, just log.
          const unknown = (msg as { kind?: string }).kind ?? 'unknown'
          console.warn('[session-events] unknown event kind:', unknown)
        }
      }
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
type AppHelloHandler = () => void
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
type FsChangedHandler = (e: FsChangedEvent) => void

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
  sessionAdded: Set<SessionAddedHandler>
  sessionRemoved: Set<SessionRemovedHandler>
  presenceChanged: Set<PresenceChangedHandler>
  openUrl: Set<OpenUrlHandler>
  projectGroupsChanged: Set<ProjectGroupsChangedHandler>
  feedbackChanged: Set<FeedbackChangedHandler>
  chatHistoryChanged: Set<ChatHistoryChangedHandler>
  tokenUsageChanged: Set<TokenUsageChangedHandler>
  fsChanged: Set<FsChangedHandler>
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
      sessionAdded: new Set(),
      sessionRemoved: new Set(),
      presenceChanged: new Set(),
      openUrl: new Set(),
      projectGroupsChanged: new Set(),
      feedbackChanged: new Set(),
      chatHistoryChanged: new Set(),
      tokenUsageChanged: new Set(),
      fsChanged: new Set(),
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

/** Files-drawer multi-writer live refresh — subscribe to APP-LEVEL
 *  `fs_changed` (paths under a workspace mutated on the host by agents,
 *  other clients, or `/cli/fs/*`). The primary bus survives host-switch
 *  WS teardown/reopen. FileTree filters `paths` / `workspacePath` against
 *  its own `rootPath`. */
export function onFsChanged(scope: ServerScope, fn: FsChangedHandler): UnsubscribeFn {
  return addHandler(busFor(scope).handlers.fsChanged, fn)
}

function dispatchAppEvent(bus: BusState, msg: SessionEventMessage): void {
  const h = bus.handlers
  switch (msg.kind) {
    case 'llm_status_changed':
      for (const fn of h.llmStatus) fn(msg)
      break
    case 'projects_changed':
      for (const fn of h.projectsChanged) fn(msg)
      break
    case 'session_activity_changed':
      for (const fn of h.sessionActivity) fn(msg)
      break
    case 'agent_status_changed':
      for (const fn of h.agentStatus) fn(msg)
      break
    case 'tunnel_status_changed':
      for (const fn of h.tunnelStatus) fn(msg)
      break
    case 'tunnel_subdomains_changed':
      for (const fn of h.tunnelSubdomains) fn(msg)
      break
    case 'publish_services_changed':
      for (const fn of h.publishServices) fn(msg)
      break
    case 'workspace_resources_changed':
      for (const fn of h.workspaceResources) fn(msg)
      break
    case 'session_added':
      for (const fn of h.sessionAdded) fn(msg)
      break
    case 'session_removed':
      for (const fn of h.sessionRemoved) fn(msg)
      break
    case 'presence_changed':
      for (const fn of h.presenceChanged) fn(msg)
      break
    case 'open_url':
      for (const fn of h.openUrl) fn(msg.url, msg.source)
      break
    case 'project_groups_changed':
      for (const fn of h.projectGroupsChanged) fn(msg.reason)
      break
    case 'feedback_changed':
      for (const fn of h.feedbackChanged) fn(msg.reason)
      break
    case 'chat_history_changed':
      for (const fn of h.chatHistoryChanged) fn()
      break
    case 'token_usage_changed':
      for (const fn of h.tokenUsageChanged) fn()
      break
    case 'fs_changed':
      for (const fn of h.fsChanged) fn(msg)
      break
    default:
      break
  }
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
  onFsChanged(fn: FsChangedHandler): UnsubscribeFn
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
    onFsChanged: (fn) => onFsChanged(scope, fn),
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
        for (const h of bus.handlers.appHello) h()
        return
      }
      if (msg.kind === 'active_changed') {
        // useActiveStore mirrors the window's server only (see
        // refreshActiveSnapshot).
        if (!scope.isPrimary) return
        useActiveStore.getState().applyActiveChanged({
          activeProjectIds: Array.isArray(msg.activeProjectIds) ? msg.activeProjectIds : [],
          activeWindowHours:
            typeof msg.activeWindowHours === 'number' ? msg.activeWindowHours : 24,
        })
        return
      }
      // Wave B APP-LEVEL broadcasts (#675) — llm/agent/tunnel. Dispatch to
      // the registries above; consumers subscribed via onLlmStatusChanged /
      // onAgentStatusChanged / onTunnelStatusChanged.
      if (
        msg.kind === 'llm_status_changed' ||
        msg.kind === 'agent_status_changed' ||
        msg.kind === 'session_activity_changed' ||
        msg.kind === 'tunnel_status_changed' ||
        msg.kind === 'tunnel_subdomains_changed' ||
        msg.kind === 'publish_services_changed' ||
        msg.kind === 'workspace_resources_changed' ||
        msg.kind === 'presence_changed' ||
        msg.kind === 'open_url' ||
        // Remote live-update fix — the project-group / feedback refetch
        // signals, plus `projects_changed` (its `onProjectsChanged`
        // registry + dispatchAppEvent case existed since 0.39.45 but the
        // kind was never listed here, so registered callbacks never
        // fired off this socket — same latent gap, fixed alongside).
        msg.kind === 'projects_changed' ||
        msg.kind === 'project_groups_changed' ||
        msg.kind === 'feedback_changed' ||
        msg.kind === 'chat_history_changed' ||
        msg.kind === 'token_usage_changed' ||
        // Files-drawer multi-writer live refresh — APP-LEVEL with paths.
        msg.kind === 'fs_changed'
      ) {
        dispatchAppEvent(bus, msg)
        return
      }
      // #688 — session_added / session_removed ALSO ride this app-level
      // socket (the daemon forwards any absolute-cwd event to the empty-
      // `?path=` subscriber). They drive the cross-workspace live-session
      // dot in `stores/active-agents.ts`; dispatch to those app-level
      // registries. The per-workspace subscriber still owns its own tab
      // adoption — this is a SEPARATE, additive consumer.
      if (msg.kind === 'session_added' || msg.kind === 'session_removed') {
        dispatchAppEvent(bus, msg)
        return
      }
      // session_renamed / review_* / tab_* / heartbeat_* — owned by the
      // per-workspace subscriber; ignore on the app-level socket.
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
      switch (msg.kind) {
        case 'hello':
          handlers.onHello?.(msg)
          break
        case 'tab_title_changed':
          handlers.onTabTitleChanged?.(msg)
          break
        case 'tab_order_changed':
          handlers.onTabOrderChanged?.(msg)
          break
        case 'heartbeat_state_changed':
          handlers.onHeartbeatStateChanged?.(msg)
          break
        case 'heartbeat_roster_changed':
          handlers.onHeartbeatRosterChanged?.(msg)
          break
        default:
          // session_added/removed/renamed + app-level + review events
          // arrive on this socket too; their dedicated subscribers own
          // them — ignore here.
          break
      }
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
