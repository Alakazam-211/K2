import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { agentDisplayName } from '@/lib/workspace-agent'
import { terminalKill } from '@/lib/terminal-daemon'
import type { MosaicNode, MosaicDirection } from 'react-mosaic-component'
import { RESUMABLE_CLI_TOOLS } from '@shared/constants'
import { useSettingsStore } from '@/stores/settings'
import { useTerminalSettingsStore, type TerminalRenderer } from '@/stores/terminal-settings'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
// Phase 2.5 fix (finding #547) — retry the workspace-layouts load
// when the daemon comes online after a slow boot.
import { onDaemonConnected } from '@/lib/daemon-reconnect'
// #625 — reset per-machine workspace-session state on a host switch.
import { onActiveHostChange } from '@/stores/connect-host'
// per-client-view-state.md — the user's SELECTED tab is per-client view
// state, sourced from this local store (never the shared layout's leaked
// `activeTabId`). Module-level fns avoid a React subscription in this store.
import { getSelectedTab, setSelectedTab, resetSelectedTabs } from '@/stores/selected-tabs'
import {
  subscribeToWorkspaceSessionEvents,
  subscribeToWorkspaceTabEvents,
  type SessionAddedEvent,
  type SessionRemovedEvent,
  type TabTitleChangedEvent,
  type TabOrderChangedEvent,
  type UnsubscribeFn,
} from '@/stores/session-events'
import { serverSupports } from '@/lib/server-capabilities'

/** Phase 2.5 fix (finding #547) — gate for `loadWorkspaceSessionsFromDb`.
 *  Flips to true on the first successful load (regardless of whether the
 *  user has any saved layouts — empty list is still a successful load).
 *  Used only to suppress the reconnect-driven retry once the baseline
 *  is established; tabs.ts doesn't gate writes the way panels/timer/
 *  focus-groups/projects do because workspace layout writes are user-
 *  driven (tab open/close/move) rather than side-effects of UI mount. */
let hasLoadedWorkspaceSessions = false

// ── Renderer reaping REMOVED (#672, daemon-canonical-active.md §4.5/§6) ──
//
// The daemon now OWNS the Active set AND the grace-reap. The renderer no
// longer schedules or fires reaps for age-out/dismiss — it is a pure
// consumer of the canonical Active mirror (useActiveStore). What used to
// live here (the 0.39.38 band-aid + the #657 reaper) is deleted:
//   - DISMISS_REAP_GRACE_MS + the `_pendingChatReaps` timer map,
//   - AgeOutSweepCandidate / AgeOutProjectMeta types,
//   - scheduleWorkspaceChatReap / cancelWorkspaceChatReap,
//   - sweepAgedOutWorkspaceChats / sweepAgedOutWorkspaceChatsFromDaemon
//     (incl. the subscriberCount gate), and the findChatSessionIdForProject
//     helper they used.
// `closeV2Session` stays — it's the deliberate tab-close path
// (closeTerminalForRenderer), not a reaper. The daemon reaper force-closes
// v2 sessions server-side.

// Lazy reference to presets store — avoids circular dependency (presets → tabs → presets).
// Set by presets.ts on init via registerPresetsStore().
let _presetsStoreRef: (() => { presets: any[] }) | null = null
export function registerPresetsStore(getter: () => { presets: any[] }): void {
  _presetsStoreRef = getter
}

// #657 — lazy reference to the projects store's `activeProjectId`,
// avoiding the projects → tabs → projects import cycle (projects.ts
// statically imports tabs.ts). projects.ts registers this getter on
// module load; the dismiss-reap path reads it at schedule/fire time.
// Falls back to `null` (treated as "no foreground") when unregistered,
// e.g. in a vitest unit that never imports projects.ts — tests can
// register a stub directly.
let _activeProjectIdRef: (() => string | null) | null = null
export function registerActiveProjectIdGetter(getter: () => string | null): void {
  _activeProjectIdRef = getter
}
function currentActiveProjectId(): string | null {
  return _activeProjectIdRef ? _activeProjectIdRef() : null
}

// #672 — lazy reference to projects.ts's `activateProject` (the canonical
// open/attach⇒activate gesture, PRD §4.3.1). Same lazy-registration
// pattern as the activeProjectId getter to avoid the projects → tabs →
// projects static import cycle. projects.ts registers this on load; the
// chat-surfacing chokepoint (`subscribeForActiveWorkspace`) calls it so
// EVERY path that surfaces a workspace chat (initial open, workspace
// restore, host-switch restore, K2 Connect remote-open, tab focus) is an
// activation — the safety property daemon-side Active-only reaping leans
// on. Falls back to a no-op when unregistered (e.g. a vitest unit that
// never imports projects.ts).
let _activateProjectRef: ((projectId: string) => void) | null = null
export function registerActivateProject(fn: (projectId: string) => void): void {
  _activateProjectRef = fn
}
function activateProjectViaRef(projectId: string): void {
  _activateProjectRef?.(projectId)
}

// ── Daemon-authoritative session events (0.38.0 Commit 4) ────────────────
//
// Pre-0.38.0 the renderer kept tabs in sync across windows by broadcasting
// `sync:tabs` Tauri events on every add/remove/title. That path is gone:
// the daemon's `/cli/sessions/events` WS now pushes the same lifecycle
// signal to every connected viewer (and to the mobile companion), and
// each window applies the events locally against its own restored layout.
//
// The active workspace's subscription handle lives here; workspace
// switches tear it down before opening the next one. See
// `subscribeForActiveWorkspace` / `tearDownActiveWorkspaceSubscription`.
let activeSessionEventsUnsub: UnsubscribeFn | null = null
let activeSessionEventsKey: string | null = null

// 0.39.39 (#676/#677) — workspace-scoped tab-title / tab-order broadcast
// subscription handle. Opened/torn-down alongside the session-events sub in
// `subscribeForActiveWorkspace` / `tearDownActiveWorkspaceSubscription` so a
// rename or reorder in one client converges on every connected client.
let activeTabEventsUnsub: UnsubscribeFn | null = null

// 0.39.39 (#677.3) — last-known layout `revision` for the active workspace
// key (the monotonic LWW token the daemon stamps on `workspace-layouts/save`).
// Captured from each save response + each load snapshot, and used to drop a
// stale local write whose base is behind a remote reorder's broadcast.
const layoutRevisions = new Map<string, number>()

/** Read the last-known layout revision for a workspace key (0 if unknown). */
export function __getLayoutRevisionForTests(key: string): number {
  return layoutRevisions.get(key) ?? 0
}

// 0.39.39 (#676/#677) regression fix — adopting a remote tab-order/layout
// reorder must be SILENT: applying the peer's layout to local state must NOT
// re-POST `workspace-layouts/save` (which would bump the revision and
// re-broadcast `TabOrderChanged`, making two clients ping-pong forever — the
// monotonic-revision LWW guard can't stop it because each adoption genuinely
// produces a higher revision). This depth counter is raised around the
// `restoreLayout` call in `refetchLayoutForRemoteReorder` and checked at the
// top of both `saveLayoutForWorkspace` and `persistActiveWorkspace`. The
// store's autosave subscription (see bottom of file) fires SYNCHRONOUSLY
// inside `restoreLayout`'s `set(...)`, so a synchronous raise/lower around
// that call suppresses the echo before the debounce timer is even scheduled.
// Legitimate user-initiated saves (drag-reorder, add/close/split tab) run
// outside this window and are unaffected.
let suppressLayoutSaveDepth = 0

/** True while a remote-reorder adoption is applying the peer's layout — the
 *  adopting client must not echo a save back to the daemon. */
function isLayoutSaveSuppressed(): boolean {
  return suppressLayoutSaveDepth > 0
}

/** Run `fn` with layout-save suppression raised, so any save triggered while
 *  adopting a peer's layout (synchronous autosave subscription or a direct
 *  `saveLayoutForWorkspace` call) is a no-op. Always lowers the depth, even on
 *  throw. */
function withLayoutSaveSuppressed<T>(fn: () => T): T {
  suppressLayoutSaveDepth++
  try {
    return fn()
  } finally {
    suppressLayoutSaveDepth--
  }
}

/** Test-only read of the suppression depth (regression test for the echo loop). */
export function __isLayoutSaveSuppressedForTests(): boolean {
  return isLayoutSaveSuppressed()
}

/** Test-only override of the last-known layout revision (#677.3 LWW). */
export function __setLayoutRevisionForTests(key: string, revision: number): void {
  layoutRevisions.set(key, revision)
}

/** Record a layout `revision` returned by `workspace-layouts/save` as our
 *  new base for the workspace key. Monotonic — never moves backwards (a
 *  late-arriving response from an earlier write can't clobber a newer base).
 *  Tolerates the old daemon's `{success}`-only response (no revision). */
function recordLayoutRevision(key: string, revision: unknown): void {
  if (typeof revision !== 'number') return
  const prev = layoutRevisions.get(key) ?? 0
  if (revision > prev) layoutRevisions.set(key, revision)
}

// Best-effort cleanup on window unload — the WS would close on its own
// when the page unmounts, but explicit close gives the daemon a clean
// disconnect notification instead of a TCP RST.
if (typeof window !== 'undefined') {
  window.addEventListener('beforeunload', () => {
    activeSessionEventsUnsub?.()
    activeSessionEventsUnsub = null
    activeSessionEventsKey = null
    activeTabEventsUnsub?.()
    activeTabEventsUnsub = null
  })
}

// ── Terminal close helpers ───────────────────────────────────────────────

/**
 * Close a terminal session on the appropriate backend for its
 * renderer. Called on every deliberate tab-close path in the
 * store (removeTab, removePaneFromTab, closeItemInPaneGroup,
 * clearAllTabs). Alacritty_v2 requires this explicit call
 * because its component unmount cleanup intentionally does NOT
 * close the daemon session — that's what makes workspace swap +
 * Tauri restart retain sessions.
 *
 * Fire-and-forget: close failures are logged but don't block the
 * UI, matching the pattern already used for terminal_kill.
 */
function closeTerminalForRenderer(data: TerminalItemData): void {
  // Heartbeat tabs are "minimize, don't kill" — the daemon-owned PTY
  // keeps running in the background after the tab closes so the
  // heartbeat continues to fire on schedule. We still flip the
  // surfaced flag so the UI knows the tab is gone, but we don't
  // call cli_sessions_v2_close (which would unregister + SIGHUP).
  // See `.k2so/prds/heartbeat-active-session-tracking.md`.
  //
  // Detection has two paths:
  //   (a) stamped metadata — SessionSurfaced flow set heartbeatName +
  //       projectPath + surfacedAgentName when the tab was created.
  //   (b) cross-reference — if the tab's args contain any heartbeat
  //       row's lastSessionId, this tab is currently running that
  //       heartbeat's session even though metadata wasn't stamped at
  //       creation time (e.g. a normal chat tab that smart_launch
  //       happened to inject into). Same close-as-minimize semantics
  //       apply: PTY survives, the row click can re-surface later.
  if (data.heartbeatName && data.projectPath && data.surfacedAgentName) {
    daemonCliPost('session/set-surfaced', {
      project_path: data.projectPath,
      agent_name: data.surfacedAgentName,
      surfaced: false,
      terminal_id: null,
      command: null,
      args: null,
      heartbeat_name: null,
      attach_agent_name: null,
    }).catch((e) => console.warn('[tabs] heartbeat surfaced=false failed:', e))
    return
  }
  // Path (b): cross-reference args against the heartbeats store.
  // Static import at module top — no circular dep with
  // heartbeat-sessions.
  if (data.args && data.args.length > 0) {
    const entries = useHeartbeatSessionsStore.getState().active
    for (const entry of entries) {
      const sid = entry.row.lastSessionId
      if (sid && data.args.includes(sid)) {
        // Tab is the heartbeat's running session — minimize, not kill.
        // We don't have a stamped surfacedAgentName, so we can't flip
        // the agent_sessions surfaced flag. That's OK — the goal is
        // PTY-survives. Skip the v2_close that would SIGHUP the child;
        // the daemon-side row's active_terminal_id stays valid; the
        // next row click finds the live session. Logs a hint so the
        // path is debuggable when it matters.
        console.info(
          '[tabs] cross-ref heartbeat tab close — leaving PTY alive (heartbeat=%s, terminalId=%s)',
          entry.row.name,
          data.terminalId,
        )
        return
      }
    }
  }

  const renderer = data.renderer ?? 'alacritty'
  switch (renderer) {
    case 'alacritty':
      // Phase 2 Unit 3 — PTY now lives in the daemon. The terminal
      // survives Tauri quit; explicit close happens via the daemon's
      // /cli/terminal/kill route.
      terminalKill(data.terminalId).catch((e) =>
        console.warn('[tabs] terminal/kill failed:', e),
      )
      break
    case 'alacritty-v2':
      // Daemon-owned PTY; unregister from v2_session_map so the
      // last Arc drops and DaemonPtySession tears down the child
      // + PTY master. See .k2so/prds/alacritty-v2.md phase A6.
      closeV2Session(`tab-${data.terminalId}`)
      break
  }
}

/**
 * GH#22 — read the live attached-client count for a pinned-chat
 * workspace from `/cli/agents/running`. The pinned-chat v2 session
 * registers under the bare projectId as its `agentName`
 * (AgentChatPane's `attachAgentName={projectId}`), so we match on
 * `agentName === projectId`. Returns the max `subscriberCount` across
 * any session sharing that name, or 0 if none / on error.
 *
 * Used at reap fire-time: if a client (remote K2 Connect viewer or the
 * host) attached during the 15s grace, the count is > 0 and the reap
 * must ABORT — closing the session would sever that live viewer.
 *
 * Fails CLOSED on error: returns -1 so the caller can choose to keep the
 * PTY warm rather than risk reaping a session whose attachment state we
 * couldn't read.
 */
async function liveSubscriberCountForProject(projectId: string): Promise<number> {
  try {
    const agents = await daemonCliGet<Array<{ agentName?: string; subscriberCount?: number }>>('agents/running')
    let max = 0
    for (const a of agents ?? []) {
      if (!a || a.agentName !== projectId) continue
      const c = typeof a.subscriberCount === 'number' ? a.subscriberCount : 0
      if (c > max) max = c
    }
    return max
  } catch (e) {
    console.warn('[tabs] agents/running attachment re-check failed:', e)
    return -1
  }
}

async function closeV2Session(agentName: string): Promise<void> {
  try {
    const creds = await getDaemonWs()
    const res = await fetch(
      `${daemonHttpBase(creds)}/cli/sessions/v2/close?token=${creds.token}`,
      {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        // force: deliberate user tab-close — bypass the daemon's attached-
        // client close-guard (GH#22 reaper defense). The user closing the tab
        // IS the attached client; the guard only exists to stop the daemon
        // reaper, which never routes through here.
        body: JSON.stringify({ agent_name: agentName, force: true }),
      },
    )
    if (!res.ok) {
      const body = await res.text()
      console.warn(
        `[tabs] v2 close ${res.status} for ${agentName}: ${body}`,
      )
    }
  } catch (e) {
    console.warn(`[tabs] v2 close failed for ${agentName}:`, e)
  }
}

/**
 * Resolve the user's enabled claude preset args (e.g.
 * --dangerously-skip-permissions). Lazy-imports the presets store so
 * module-load doesn't cascade through settings.ts (which calls invoke
 * and breaks vitest environments without a window). Strips any
 * pre-existing `--resume` so callers can append their own deterministically.
 * Returns `[]` when no enabled claude preset is configured.
 */
async function resolveClaudePresetArgs(): Promise<string[]> {
  try {
    const { usePresetsStore } = await import('@/stores/presets')
    const presets = usePresetsStore.getState().presets
    const preset = presets.find((p) => p.command.split(/\s+/)[0] === 'claude' && p.enabled)
    if (!preset) return []
    const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
    const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
    return cleaned.slice(1).filter((a: string) => a !== '--resume')
  } catch {
    return []
  }
}

// ── Item Types ────────────────────────────────────────────────────────────

export interface TerminalItemData {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  sessionId?: string  // CLI tool session ID for resume on restart
  /** performance.now() timestamp captured at the moment the user
   *  pressed Cmd+T / Cmd+Shift+T / Cmd+D to create this terminal.
   *  Terminal view components compare to performance.now() at their
   *  first-content render to report end-to-end spawn→visible time.
   *  Backends can measure themselves against this for an apples-to-
   *  apples comparison between renderers. */
  spawnedAt?: number
  /**
   * Renderer selection — captured at tab creation from the user's
   * terminal settings preference. Each tab remembers its own
   * renderer so toggling the preference doesn't hot-swap existing
   * terminals mid-session. Missing / undefined = alacritty (the
   * historical default for every tab created pre-4.5).
   *
   *   - 'alacritty'     (Legacy) — Tauri-local PTY (terminal_kill).
   *   - 'alacritty-v2'  — daemon-owned PTY (cli/sessions/v2/close).
   */
  renderer?: 'alacritty' | 'alacritty-v2'
  /** Set on tabs that are attached to a heartbeat's live PTY.
   *  Closing the tab flips `surfaced=false` instead of killing the
   *  PTY (the heartbeat keeps firing in the background).
   *  See `.k2so/prds/heartbeat-active-session-tracking.md`. */
  heartbeatName?: string
  /** Override the v2_session_map agent_name TerminalPane uses on
   *  /cli/sessions/v2/spawn. Necessary for surfaced heartbeat tabs:
   *  the daemon registered the existing PTY under the workspace's
   *  primary agent name, not the renderer's default
   *  `tab-${terminalId}`. Without this we'd spawn a duplicate PTY
   *  instead of attaching. */
  attachAgentName?: string
  /** Workspace path for the heartbeat — needed at tab-close time
   *  to call k2so_session_set_surfaced. Mirror of TerminalItemData.cwd
   *  in most cases, but explicit so tabs that change cwd can't
   *  desync. */
  projectPath?: string
  /** Agent name on the agent_sessions row whose `surfaced` flag is
   *  toggled when this tab opens / closes. */
  surfacedAgentName?: string
}

export interface FileViewerItemData {
  filePath: string
  /** When 'diff', shows unified diff view instead of editor */
  mode?: 'edit' | 'diff'
  /** Saved scroll position (pixels) — restored on tab re-activation */
  scrollTop?: number
  /** Saved cursor position in the editor (character offset) */
  cursorPos?: number
}

export interface AgentItemData {
  agentName: string
  projectPath: string
  /** Which section of the agent UI this pinned tab renders.
   *  'inbox' = work-queue kanban; 'chat' = persistent Claude chat.
   *  Pre-0.36.0 a single AgentPane held both as sub-tabs; the split
   *  promotes them to two top-level pinned tabs. Optional for
   *  backwards-compat with pre-split serialized rows (defaults to
   *  'inbox' on restore so existing tabs land on the work board). */
  section?: 'inbox' | 'chat'
  /** 0.37.12 — Claude session UUID for the pinned chat tab.
   *  Persisted in the serialized layout so on close→reopen the
   *  AgentChatPane immediately resumes the **same** session
   *  without needing a daemon roundtrip that can race or miss.
   *  Set on `chat` section only — `inbox` tabs don't carry a
   *  Claude session of their own.
   *
   *  Stamped by `AgentChatPane` after it resolves its launch
   *  config (either from a restored hint or from
   *  `k2so_agents_resume_chat_args`). See
   *  `.k2so/prds/canonical-lane-restore.md`. */
  sessionId?: string
}

export interface Item {
  id: string
  type: 'terminal' | 'file-viewer' | 'agent'
  data: TerminalItemData | FileViewerItemData | AgentItemData
  pinned?: boolean
}

// ── PaneGroup ─────────────────────────────────────────────────────────────

export interface PaneGroup {
  id: string
  items: Item[]
  activeItemIndex: number
}

// ── Legacy compat types (re-exported for consumers) ──────────────────────

export interface TerminalPaneData {
  type: 'terminal'
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
}

export interface FileViewerPaneData {
  type: 'file-viewer'
  filePath: string
  pinned: boolean
}

/** @deprecated Use FileViewerPaneData instead */
export interface MarkdownPaneData {
  type: 'markdown'
  filePath: string
}

export type PaneData = TerminalPaneData | FileViewerPaneData | { type: 'agent'; agentName: string; projectPath: string; section?: 'inbox' | 'chat' }

// Keep backward-compat alias
export type TerminalPane = TerminalPaneData

// ── Serialization Types ─────────────────────────────────────────────────

/** 0.38.0 layout schema version stamped on serialize. Readers fall back
 *  to v1 semantics when the field is missing or `< 2`. v1 → v2 migration
 *  happens in-place on read; the daemon also runs a one-shot boot pass
 *  (`workspace_layouts_dedup::run_once`) so disk rows converge eagerly. */
export const LAYOUT_SCHEMA_VERSION = 2

/** Discriminated union for one serialized item inside a paneGroup.
 *
 *  Terminal items split by version:
 *   - v1 (legacy): carried daemon-owned data (cwd / command / args /
 *     sessionId / renderer). Kept readable for migration; never emitted.
 *   - v2 (current): paneGroupId is the canonical key (daemon's
 *     `tab-<paneGroupId>` agent_name). Daemon owns cwd/command/args/
 *     sessionId/renderer — those come back via
 *     `k2so_sessions_list_for_workspace` in `reconcileWithDaemon`.
 *
 *  Heartbeat metadata fields (`heartbeatName`, `surfacedAgentName`,
 *  `attachAgentName`, `projectPath`) stay on v2 terminals — the daemon's
 *  list endpoint doesn't currently expose them and they're load-bearing
 *  for close-as-minimize behavior in `closeTerminalForRenderer`.
 *  Removing them is a follow-up once the daemon API grows. */
export interface SerializedTerminalItemV2 {
  id: string
  type: 'terminal'
  /** The canonical key — equals the parent paneGroup id and (modulo
   *  the `tab-` prefix) the daemon's agent_name. Daemon is source of
   *  truth for command/args/cwd/sessionId; reconcile fills them in. */
  paneGroupId: string
  /** Load-bearing for close-as-minimize. See doc comment on the type. */
  heartbeatName?: string
  /** Load-bearing for close-as-minimize. See doc comment on the type. */
  surfacedAgentName?: string
  /** Load-bearing for close-as-minimize. See doc comment on the type. */
  attachAgentName?: string
  /** Load-bearing for close-as-minimize. See doc comment on the type. */
  projectPath?: string
}

/** Legacy v1 terminal shape — read-only, never emitted by 0.38.0+.
 *  Tolerated by readers via the v1→v2 migration in `restoreLayout`. */
export interface SerializedTerminalItemV1 {
  id: string
  type: 'terminal'
  cwd?: string
  command?: string
  args?: string[]
  sessionId?: string
  renderer?: 'alacritty' | 'alacritty-v2'
  heartbeatName?: string
  surfacedAgentName?: string
  attachAgentName?: string
  projectPath?: string
}

export interface SerializedAgentItem {
  id: string
  type: 'agent'
  agentName?: string
  projectPath?: string
  /** Which section the system-pinned agent tab renders.
   *  See AgentItemData.section for semantics. Defaults to 'inbox'
   *  on deserialize when missing (legacy layouts). */
  section?: 'inbox' | 'chat'
  /** 0.37.12 — Claude session UUID for the pinned chat tab.
   *  Persisted so close→reopen resumes the same conversation
   *  without a daemon roundtrip race. Chat section only. */
  sessionId?: string
}

export interface SerializedFileViewerItem {
  id: string
  type: 'file-viewer'
  filePath?: string
  pinned?: boolean
  /** Saved scroll / cursor position (file-viewer only). */
  scrollTop?: number
  cursorPos?: number
}

/** Union of all serialized item shapes accepted by `restoreLayout`.
 *  Includes the legacy v1 terminal shape so pre-0.38.0 layouts deserialize
 *  cleanly; `restoreLayout` migrates them to v2 before constructing Tab
 *  objects. */
export type SerializedItem =
  | SerializedTerminalItemV2
  | SerializedTerminalItemV1
  | SerializedAgentItem
  | SerializedFileViewerItem

export interface SerializedPaneGroup {
  id: string
  items: SerializedItem[]
  activeItemIndex: number
}

export interface SerializedTab {
  id: string
  title: string
  mosaicTree: MosaicNode<string> | null
  paneGroups: Record<string, SerializedPaneGroup>
  isSystemAgent?: boolean
  /** #587 — pinned HTML file tab. Rendered immediately after the
   *  system (Chat/Inbox) tabs and before regular tabs. Survives reload
   *  via the same serialize/restore path as system tabs; closing one
   *  unpins (removes) it rather than hiding. */
  isPinnedFile?: boolean
}

export interface SerializedLayout {
  /** Schema version. Absent / < 2 = v1; readers migrate to v2 in-place. */
  version?: number
  tabs: SerializedTab[]
  /** @deprecated per-client-view-state.md — selected tab is per-client VIEW
   *  state, NOT canonical. No longer written into the shared layout (it was
   *  leaking one client's selection onto peers via `workspace-layouts/save`).
   *  Still typed optional so OLD stored layouts (and older daemons) parse;
   *  readers IGNORE it for selection (selection comes from the per-client
   *  selected-tabs store). */
  activeTabId?: string | null
  extraGroups?: Array<{ tabs: SerializedTab[], activeTabId?: string | null }>
  splitCount?: number
  activeGroupIndex?: number
}

// ── Background workspace snapshot (live tabs with running PTYs) ─────────

export interface WorkspaceTabSnapshot {
  tabs: Tab[]
  extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }>
  splitCount: number
  activeGroupIndex: number
  activeTabId: string | null
}

// ── Tab ─────────────────────────────────────────────────────────────────

export interface Tab {
  id: string
  title: string
  mosaicTree: MosaicNode<string> | null  // leaf strings = paneGroupIds
  paneGroups: Map<string, PaneGroup>
  isDirty?: boolean
  /** System agent tab — pinned at start of tab bar, can't be closed or reordered */
  isSystemAgent?: boolean
  /** #587 — pinned HTML file tab. Sits right after the system (Chat/
   *  Inbox) tabs and before regular tabs. Carries a pinned file-viewer
   *  item (html mode). Closing it unpins rather than hides. */
  isPinnedFile?: boolean
}

interface TabsState {
  tabs: Tab[]
  activeTabId: string | null

  // Existing actions (signatures preserved)
  addTab: (cwd: string, options?: { title?: string; command?: string; args?: string[] }) => string
  removeTab: (tabId: string) => void
  setActiveTab: (tabId: string) => void
  splitPane: (
    tabId: string,
    existingPaneId: string,
    newPaneId: string,
    newPane: TerminalPaneData,
    direction: MosaicDirection
  ) => void
  updateMosaicTree: (tabId: string, tree: MosaicNode<string> | null) => void
  reorderTabs: (fromIndex: number, toIndex: number, groupIndex?: number) => void
  addPaneToTab: (tabId: string, paneId: string, pane: PaneData) => void
  removePaneFromTab: (tabId: string, paneGroupId: string) => void
  moveItemBetweenPanes: (fromTabId: string, fromPaneGroupId: string, itemId: string, toTabId: string, toPaneGroupId: string) => void
  getActiveTab: () => Tab | undefined
  openFileInPane: (tabId: string, filePath: string) => void
  /** 0.37.12 — stamp the canonical Claude session id on an agent
   *  item's data so the next `serializeTab` captures it. Called by
   *  `AgentChatPane` after it resolves the session_id (either from
   *  a restored hint or from `k2so_agents_resume_chat_args`). The
   *  serialized layout becomes the renderer-side canonical record,
   *  surviving daemon DB races + crash windows. See
   *  `.k2so/prds/canonical-lane-restore.md`.
   *
   *  GH#608: `ownerProjectId` is the canonical id of the workspace whose
   *  pinned chat spawned this session (AgentChatPane resolves it
   *  synchronously from the projects store). `state.tabs` only ever holds
   *  the ACTIVE workspace's tabs, so a stamp originating from a stale /
   *  background chat pane (same agentName+projectPath, different
   *  workspace) must NOT land on the active workspace's chat item. The
   *  stamp is dropped unless `ownerProjectId` matches the project that
   *  `activeWorkspaceKey` is bound to (GH#679 — was the cross-store
   *  projects-active id, which raced and dropped legit stamps). */
  stampAgentSessionId: (agentName: string, projectPath: string, sessionId: string, ownerProjectId: string) => void
  openAgentPane: (agentName: string, projectPath: string, title?: string) => void
  /** Open a tab bound to a specific heartbeat's chat session, or focus
   *  the existing tab if one is already open. Resolves the launch
   *  config (incl. --resume on agent_heartbeats.last_session_id) via
   *  k2so_agents_build_launch with the heartbeat name. Returns the
   *  tab id that was opened or focused. */
  openHeartbeatTab: (
    projectPath: string,
    heartbeatName: string,
    options?: { existingTerminalId?: string },
  ) => Promise<string | null>
  openFileAsTab: (filePath: string) => void
  openFileInPaneGroup: (tabId: string, paneGroupId: string, filePath: string) => void
  openDiffInPane: (tabId: string, filePath: string) => void
  pinPane: (tabId: string, paneGroupId: string) => void
  unpinPane: (tabId: string, paneGroupId: string) => void
  openFileInNewTab: (filePath: string) => void
  openUntitledDocument: (cwd: string) => void
  setTabTitle: (tabId: string, title: string) => void
  /** 0.39.39 (#676) — apply a daemon-canonical title to a tab WITHOUT
   *  re-POSTing (used by the `tab_title_changed` broadcast handler + the
   *  on-load `tab-titles` snapshot, so a rename in another client shows
   *  here). No-op when the tab id isn't currently surfaced. */
  applyDaemonTabTitle: (tabId: string, title: string) => void
  renameTabByTitle: (oldTitle: string, newTitle: string) => void
  setTabDirty: (tabId: string, dirty: boolean) => void
  setFileViewerState: (tabId: string, paneId: string, itemId: string, state: { scrollTop?: number; cursorPos?: number }) => void
  /** @deprecated Use openFileInPane instead */
  openMarkdownPane: (tabId: string, filePath: string, splitDirection?: 'row' | 'column') => void

  // NEW: PaneGroup item management
  addItemToPaneGroup: (tabId: string, paneGroupId: string, item: Item) => void
  activateItemInPaneGroup: (tabId: string, paneGroupId: string, itemIndex: number) => void
  closeItemInPaneGroup: (tabId: string, paneGroupId: string, itemId: string) => void
  getActivePaneGroupId: (tabId: string) => string | null

  // Split the active tab's panes (up to max panes within a tab)
  splitActivePane: (cwd: string, maxPanes?: number) => boolean

  // Get the number of panes in the active tab
  getActivePaneCount: () => number

  // ── Tab Groups (independent columns, each with own tab bar) ──────
  splitCount: number  // 1, 2, or 3 columns
  extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }>
  activeGroupIndex: number  // which group receives new tabs by default

  splitTerminalArea: (cwd: string) => void    // add a column (max 3)
  unsplitTerminalArea: () => void              // remove rightmost column
  setActiveGroup: (index: number) => void
  addTabToGroup: (groupIndex: number, cwd: string, options?: { title?: string; command?: string; args?: string[] }) => string
  removeTabFromGroup: (groupIndex: number, tabId: string) => void
  setActiveTabInGroup: (groupIndex: number, tabId: string) => void
  moveTabToGroup: (fromGroup: number, toGroup: number, tabId: string) => void
  getGroupTabs: (groupIndex: number) => { tabs: Tab[], activeTabId: string | null }

  // Navigation history (back/forward)
  navHistory: string[]     // stack of tabIds
  navIndex: number         // current position (-1 = no history)
  canGoBack: () => boolean
  canGoForward: () => boolean
  goBack: () => void
  goForward: () => void

  // Layout persistence per workspace
  activeWorkspaceKey: string | null  // "projectId:workspaceId" for auto-save
  // per-client-view-state.md — the project/workspace ids of the active
  // workspace, threaded in by `loadLayoutForWorkspace`/`restoreWorkspace`
  // so the selected-tab reads/writes (which run inside `restoreLayout` /
  // `setActiveTabInGroup`, neither of which receives the ids as args) can
  // key the per-client selected-tabs store. Kept in lockstep with
  // `activeWorkspaceKey` (which is `${projectId}:${workspaceId}`), but split
  // explicitly so a colon in a projectId can't corrupt the key.
  activeProjectId: string | null
  activeWorkspaceId: string | null
  backgroundWorkspaces: Record<string, WorkspaceTabSnapshot>
  workspaceLayouts: Record<string, SerializedLayout>
  serializeCurrentLayout: () => SerializedLayout
  restoreLayout: (layout: SerializedLayout, cwd: string) => void
  saveLayoutForWorkspace: (projectId: string, workspaceId: string) => void
  /** Restore the saved tab layout for a workspace into the active view.
   *  Returns a promise that resolves once the *initial* restore has run
   *  (`restoreLayout` synchronously sets tabs + activeTabId + sessionId,
   *  or `launchDefaultAgent` seeds the empty-workspace case). The daemon
   *  reconcile pass runs in the background and is NOT awaited. Cold-boot
   *  callers (#658) await this before `ensurePinnedAgentTabForMode` so
   *  the restored Chat tab — with its activeTabId + sessionId — wins the
   *  ordering race against the pinned-tab ensure. */
  loadLayoutForWorkspace: (projectId: string, workspaceId: string, cwd: string) => Promise<void>
  loadWorkspaceSessionsFromDb: () => Promise<void>
  /** @deprecated Use loadWorkspaceSessionsFromDb instead */
  loadWorkspaceLayoutsFromSettings: () => Promise<void>
  clearAllTabs: () => void
  detectAndSaveSessionIds: () => Promise<void>

  // Background workspace management
  launchDefaultAgent: (key: string, cwd: string) => void
  stashWorkspace: (key: string) => void
  restoreWorkspace: (key: string, cwd: string) => Promise<void>
  serializeAllWorkspaces: (activeKey: string) => Promise<void>
  clearBackgroundWorkspace: (key: string) => void
  // #672 — the renderer reaper API (scheduleWorkspaceChatReap /
  // cancelWorkspaceChatReap / sweepAgedOutWorkspaceChats /
  // sweepAgedOutWorkspaceChatsFromDaemon) was REMOVED. The daemon owns
  // reaping now (daemon-canonical-active.md §4.5). Do not re-introduce a
  // renderer reap — it would race the daemon's canonical decision.
  persistActiveWorkspace: () => void
  /** Add a tab to a workspace without switching to it. If the workspace is active,
   *  adds directly. If background/stashed, saves to DB session so it's there when restored.
   *  Returns the terminal ID (paneGroupId) so the caller can spawn a background PTY. */
  addTabToWorkspace: (workspaceKey: string, cwd: string, options: { title: string; command: string; args: string[] }) => string | null

  // Pinned system agent tab
  /** Ensure a pinned agent tab exists for this workspace. Creates one if missing.
   *  Returns the tab ID. No-op if tab already exists. */
  ensureSystemAgentTabs: (agentName: string, projectPath: string, title: string) => string
  /** Remove the pinned system agent tab (when agent mode is turned off). */
  removeSystemAgentTab: () => void

  // ── Pinned HTML file tabs (#587) ─────────────────────────────────
  /** Pin an HTML file as a top-level tab. The pinned tab renders the
   *  file in FileViewerPane (html mode, rendered by default) and sits
   *  immediately after the system (Chat/Inbox) tabs, before regular
   *  tabs. Per-workspace (lives in this workspace's tab list, which is
   *  serialized/restored per workspace). No-op if already pinned;
   *  focuses the existing tab instead. */
  pinFileAsTab: (filePath: string) => void
  /** Unpin a pinned HTML file tab (also used as the close=unpin path). */
  unpinFileTab: (filePath: string) => void
  /** True if `filePath` is currently pinned as a top-level tab in the
   *  active workspace. */
  isFilePinned: (filePath: string) => boolean
  /** Activate the pinned system agent tab (switches to it). */
  activateSystemAgentTab: () => void
  /** Get the pinned system agent tab if it exists. */
  getSystemAgentTab: () => Tab | undefined

  // 0.38.0 Commit 4 — cross-window tab sync retired. Daemon push via
  // `/cli/sessions/events` is the new source of truth; see
  // `subscribeForActiveWorkspace` below the store body.
}

// ── Helpers ──────────────────────────────────────────────────────────────

let tabCounter = 0

/** Reconcile a restored system agent tab to the current workspace.
 *  Returns the same tab reference when nothing changed (cheap, no
 *  needless re-render); otherwise a shallow-copied tab whose agent
 *  item(s) carry the authoritative agentName/projectPath. Drops the
 *  chat sessionId when projectPath changes — that session was the old
 *  workspace's. See the call site in `ensureSystemAgentTabs`. */
function reconcileSystemAgentTab(tab: Tab, agentName: string, projectPath: string): Tab {
  let mutated = false
  const newPaneGroups = new Map<string, PaneGroup>()
  for (const [pgId, pg] of tab.paneGroups) {
    let pgChanged = false
    const newItems = pg.items.map((item) => {
      if (item.type !== 'agent') return item
      const d = item.data as AgentItemData
      if (d.agentName === agentName && d.projectPath === projectPath) return item
      pgChanged = true
      const pathChanged = d.projectPath !== projectPath
      return {
        ...item,
        data: {
          ...d,
          agentName,
          projectPath,
          sessionId: pathChanged ? undefined : d.sessionId,
        },
      }
    })
    if (pgChanged) mutated = true
    newPaneGroups.set(pgId, pgChanged ? { ...pg, items: newItems } : pg)
  }
  return mutated ? { ...tab, paneGroups: newPaneGroups } : tab
}

/** Ensure the pinned system agent tab matches the given agent mode.
 *  Called by the projects store after workspace restore/switch.
 *  Resolves the primary agent name from the backend asynchronously. */
export function ensurePinnedAgentTabForMode(
  agentMode: string,
  projectPath: string,
): void {
  // Capture the workspace this call is FOR. setActiveWorkspace sets
  // activeWorkspaceKey (via restoreWorkspace) synchronously before
  // calling us, so this is the workspace the caller intends to pin
  // tabs for. We re-check it after the async agent-name resolution
  // below and bail if the user switched away in the meantime — see
  // the guard before ensureSystemAgentTabs.
  const expectedWorkspaceKey = useTabsStore.getState().activeWorkspaceKey
  setTimeout(async () => {
    const tabsStore = useTabsStore.getState()

    // 0.39.0: Chat + Inbox tabs render for EVERY workspace, including
    // ones with agentMode === 'off'. Reason: per the workspace==agent
    // model, every workspace is an agent that other workspaces can
    // message via `k2so msg <workspace>` regardless of whether the
    // user has agent automation actively running here. The Inbox tab
    // shows incoming cross-workspace messages; the Chat tab spawns a
    // CLI session (Claude Code, Codex, etc.) against this workspace
    // when clicked. Pre-0.39.0 these tabs were hidden when agent mode
    // was off — that hid the inbox/chat surface even though the
    // underlying capability was always present.

    let title = 'Agent'
    if (agentMode === 'manager' || agentMode === 'coordinator') {
      title = 'Manager'
    } else if (agentMode === 'custom') {
      title = 'Agent'
    } else if (agentMode === 'agent') {
      title = 'K2'
    } else if (!agentMode || agentMode === 'off') {
      title = 'Workspace'  // agent-mode-off: workspace is its own agent
    }

    // Resolve the actual primary agent name from the backend. The
    // daemon's `agent_display_name` helper is total (always returns
    // a string — folder basename if AGENT.md is missing), so the
    // pinned tab gets a routable identity even when the agent list
    // call fails. Post-0.39.0f Phase 2.1: no `__lead__` fallback —
    // the daemon owns primary-agent resolution end-to-end.
    const { invoke } = await import('@tauri-apps/api/core')
    let agentName = ''
    try {
      const agents = await invoke<Array<{ name: string; isManager: boolean; agentType: string }>>('k2so_agents_list', { projectPath })
      if (agents && agents.length > 0) {
        // Find the primary agent: manager/coordinator first, then first custom, then first agent
        const manager = agents.find((a) => a.isManager || a.agentType === 'manager' || a.agentType === 'coordinator')
        const custom = agents.find((a) => a.agentType === 'custom')
        const k2so = agents.find((a) => a.agentType === 'k2so')
        if (agentMode === 'manager' || agentMode === 'coordinator') {
          agentName = manager?.name ?? agents[0].name
        } else if (agentMode === 'custom') {
          agentName = custom?.name ?? agents[0].name
        } else if (agentMode === 'agent') {
          agentName = k2so?.name ?? agents[0].name
        }
      }
    } catch {
      // Fall through to the display-name resolver below.
    }
    if (!agentName) {
      try {
        agentName = await agentDisplayName(projectPath)
        if (!agentName) throw new Error('empty display name')
      } catch {
        // As an absolute last resort use the project path basename so
        // we never emit an empty routing key. The daemon will refuse
        // the unknown name loudly when it's used.
        agentName = projectPath.split('/').filter(Boolean).pop() ?? 'agent'
      }
    }

    // Active-workspace guard. agent-name resolution above is async
    // (a setTimeout plus an awaited k2so_agents_list round-trip), so
    // the user can switch workspaces while this call is in flight.
    // ensureSystemAgentTabs mutates the GLOBALLY-active tab set, so a
    // stale callback would stamp THIS workspace's agent/projectPath
    // into whichever workspace is now active — e.g. switching from a
    // K2 worktree (agent `cli-eng`, path `…/K2`) into HK47 mid-
    // resolution writes `cli-eng`/`…/K2` into HK47's pinned tabs.
    // Bail if the active workspace changed since we were scheduled.
    if (useTabsStore.getState().activeWorkspaceKey !== expectedWorkspaceKey) return
    tabsStore.ensureSystemAgentTabs(agentName, projectPath, title)
  }, 0)
}

/** Serialize a single Tab to SerializedTab (used by serializeCurrentLayout and serializeAllWorkspaces).
 *  Emits the v2 terminal shape: paneGroupId is the canonical key; daemon
 *  owns cwd/command/args/sessionId/renderer (those come back via
 *  `reconcileWithDaemon` at restore time, not from layout JSON). */
function serializeTab(tab: Tab): SerializedTab {
  const paneGroupsObj: Record<string, SerializedPaneGroup> = {}
  for (const [pgId, pg] of tab.paneGroups) {
    const serializedItems: SerializedItem[] = pg.items.map((item) => {
      if (item.type === 'terminal') {
        const d = item.data as TerminalItemData
        const v2: SerializedTerminalItemV2 = {
          id: item.id,
          type: 'terminal' as const,
          paneGroupId: pgId,
          // Heartbeat metadata stays on v2 — daemon's list endpoint
          // doesn't currently expose these, and they're load-bearing
          // for close-as-minimize semantics in
          // `closeTerminalForRenderer`. Removing them is a follow-up
          // once the daemon API grows.
          heartbeatName: d.heartbeatName,
          projectPath: d.projectPath,
          surfacedAgentName: d.surfacedAgentName,
          attachAgentName: d.attachAgentName,
        }
        return v2
      } else if (item.type === 'agent') {
        const d = item.data as AgentItemData
        return {
          id: item.id,
          type: 'agent' as const,
          agentName: d.agentName,
          projectPath: d.projectPath,
          section: d.section,
          // 0.37.12 — pinned chat tab carries its canonical Claude
          // session id so close → reopen resumes the same conversation
          // without a daemon roundtrip race. See PRD
          // .k2so/prds/canonical-lane-restore.md.
          sessionId: d.sessionId,
        }
      } else {
        const d = item.data as FileViewerItemData
        return {
          id: item.id,
          type: 'file-viewer' as const,
          filePath: d.filePath,
          pinned: item.pinned,
          scrollTop: d.scrollTop,
          cursorPos: d.cursorPos,
        }
      }
    })
    paneGroupsObj[pgId] = {
      id: pg.id,
      items: serializedItems,
      activeItemIndex: Math.min(pg.activeItemIndex, Math.max(0, pg.items.length - 1)),
    }
  }
  return {
    id: tab.id,
    title: tab.title,
    mosaicTree: tab.mosaicTree,
    paneGroups: paneGroupsObj,
    ...(tab.isSystemAgent ? { isSystemAgent: true } : {}),
    ...(tab.isPinnedFile ? { isPinnedFile: true } : {}),
  }
}

/** Serialize a WorkspaceTabSnapshot (background workspace with live tabs).
 *  per-client-view-state.md (Phase 1) — like `serializeCurrentLayout`, the
 *  canonical background-workspace layout carries STRUCTURE only; the snapshot's
 *  `activeTabId` (top-level + per-split-group) is per-client VIEW state and is
 *  intentionally omitted so a background-workspace save can't leak this
 *  client's selection to peers. */
function serializeSnapshot(snapshot: WorkspaceTabSnapshot): SerializedLayout {
  const serializedExtraGroups = snapshot.extraGroups.map((group) => ({
    tabs: group.tabs.map(serializeTab),
  }))
  return {
    version: LAYOUT_SCHEMA_VERSION,
    tabs: snapshot.tabs.map(serializeTab),
    extraGroups: serializedExtraGroups.length > 0 ? serializedExtraGroups : undefined,
    splitCount: snapshot.splitCount > 1 ? snapshot.splitCount : undefined,
    activeGroupIndex: snapshot.activeGroupIndex > 0 ? snapshot.activeGroupIndex : undefined,
  }
}

/** Debounce timer for auto-saving the active workspace. */
let persistDebounceTimer: ReturnType<typeof setTimeout> | null = null

/** Cancel a pending debounced autosave (see `persistActiveWorkspace`). Used by
 *  the silent remote-reorder adoption path (#676/#677) so a pre-adoption save
 *  scheduled before suppression can't fire ~1s later and serialize the
 *  just-adopted layout, re-emitting the echo we suppressed. */
function cancelPendingLayoutSave(): void {
  if (persistDebounceTimer) {
    clearTimeout(persistDebounceTimer)
    persistDebounceTimer = null
  }
}

function removePaneFromTree(
  tree: MosaicNode<string> | null,
  paneId: string
): MosaicNode<string> | null {
  if (tree === null) return null
  if (typeof tree === 'string') {
    return tree === paneId ? null : tree
  }

  const newFirst = removePaneFromTree(tree.first, paneId)
  const newSecond = removePaneFromTree(tree.second, paneId)

  if (newFirst === null && newSecond === null) return null
  if (newFirst === null) return newSecond
  if (newSecond === null) return newFirst

  return { ...tree, first: newFirst, second: newSecond }
}

function getFirstLeaf(tree: MosaicNode<string> | null): string | null {
  if (tree === null) return null
  if (typeof tree === 'string') return tree
  return getFirstLeaf(tree.first)
}

/** Count leaf nodes (panes) in a mosaic tree */
function countLeaves(tree: MosaicNode<string> | null): number {
  if (tree === null) return 0
  if (typeof tree === 'string') return 1
  return countLeaves(tree.first) + countLeaves(tree.second)
}

/** Find a tab across all groups (group 0 = main tabs, groups 1+ = extraGroups) */
function findTabAcrossGroups(state: { tabs: Tab[], extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> }, tabId: string): Tab | undefined {
  const found = state.tabs.find((t) => t.id === tabId)
  if (found) return found
  for (const group of state.extraGroups) {
    const f = group.tabs.find((t) => t.id === tabId)
    if (f) return f
  }
  return undefined
}

/** Apply a mapping function to a tab wherever it lives (group 0 or extraGroups) */
function mapTabAcrossGroups(
  state: { tabs: Tab[], extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> },
  tabId: string,
  fn: (tab: Tab) => Tab
): { tabs: Tab[], extraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> } {
  // Check group 0
  const idx0 = state.tabs.findIndex((t) => t.id === tabId)
  if (idx0 >= 0) {
    return {
      tabs: state.tabs.map((t) => t.id === tabId ? fn(t) : t),
      extraGroups: state.extraGroups
    }
  }
  // Check extra groups
  for (let gi = 0; gi < state.extraGroups.length; gi++) {
    const idx = state.extraGroups[gi].tabs.findIndex((t) => t.id === tabId)
    if (idx >= 0) {
      const newGroups = [...state.extraGroups]
      newGroups[gi] = {
        ...newGroups[gi],
        tabs: newGroups[gi].tabs.map((t) => t.id === tabId ? fn(t) : t)
      }
      return { tabs: state.tabs, extraGroups: newGroups }
    }
  }
  return { tabs: state.tabs, extraGroups: state.extraGroups }
}

/** Read the current user-selected renderer. Snapshot-at-call-time:
 *  tabs stamp the renderer they see AT CREATION, so toggling the
 *  setting mid-session doesn't hot-swap open panes — only new ones
 *  pick up the change. Safe to call from anywhere in the renderer
 *  process (every terminal-tab creation path below goes through this). */
function currentRenderer(): TerminalRenderer {
  try {
    return useTerminalSettingsStore.getState().renderer
  } catch {
    // Store not initialized (SSR/tests) — Alacritty is the safe default.
    return 'alacritty'
  }
}

/** Create a PaneGroup with a single terminal item */
function makeTerminalPaneGroup(
  paneGroupId: string,
  cwd: string,
  options?: { command?: string; args?: string[] }
): PaneGroup {
  const itemId = crypto.randomUUID()
  return {
    id: paneGroupId,
    items: [
      {
        id: itemId,
        type: 'terminal',
        data: {
          terminalId: paneGroupId, // use paneGroupId as terminalId for compat
          cwd,
          command: options?.command,
          args: options?.args,
          renderer: currentRenderer(),
          spawnedAt: performance.now(),
        },
      },
    ],
    activeItemIndex: 0,
  }
}

/** Create a PaneGroup with a single file-viewer item */
function makeFileViewerPaneGroup(
  paneGroupId: string,
  filePath: string,
  pinned: boolean
): PaneGroup {
  const itemId = crypto.randomUUID()
  return {
    id: paneGroupId,
    items: [
      {
        id: itemId,
        type: 'file-viewer',
        data: { filePath },
        pinned,
      },
    ],
    activeItemIndex: 0,
  }
}

/** Convert a PaneData to an Item (for backward compat in addPaneToTab) */
function paneDataToItem(pane: PaneData): Item {
  if (pane.type === 'terminal') {
    return {
      id: crypto.randomUUID(),
      type: 'terminal',
      data: {
        terminalId: pane.terminalId,
        cwd: pane.cwd,
        command: pane.command,
        args: pane.args,
        // Splits / paneDataToItem were silently dropping the renderer
        // field, which meant Cmd+D (split pane) always landed in
        // Alacritty even when the user had a different renderer selected.
        // Snapshot the current setting for consistency with makeTerminalPaneGroup.
        renderer: currentRenderer(),
        spawnedAt: performance.now(),
      },
    }
  } else if (pane.type === 'agent') {
    return {
      id: crypto.randomUUID(),
      type: 'agent',
      data: { agentName: pane.agentName, projectPath: pane.projectPath },
    }
  } else {
    return {
      id: crypto.randomUUID(),
      type: 'file-viewer',
      data: { filePath: pane.filePath },
      pinned: pane.pinned,
    }
  }
}

/**
 * Get the active item of a PaneGroup projected as a flat PaneData.
 * Useful for backward-compat reads in consuming components.
 */
export function paneGroupToActivePaneData(pg: PaneGroup): PaneData {
  const item = pg.items[pg.activeItemIndex] ?? pg.items[0]
  if (item.type === 'terminal') {
    const d = item.data as TerminalItemData
    return {
      type: 'terminal',
      terminalId: d.terminalId,
      cwd: d.cwd,
      command: d.command,
      args: d.args,
    }
  } else if (item.type === 'agent') {
    const d = item.data as AgentItemData
    return {
      type: 'agent',
      agentName: d.agentName,
      projectPath: d.projectPath,
      section: d.section,
    }
  } else {
    const d = item.data as FileViewerItemData
    return {
      type: 'file-viewer',
      filePath: d.filePath,
      pinned: item.pinned ?? false,
    }
  }
}

// ── Store ────────────────────────────────────────────────────────────────

export const useTabsStore = create<TabsState>((set, get) => ({
  tabs: [],
  activeTabId: null,
  splitCount: 1,
  extraGroups: [],
  activeGroupIndex: 0,
  navHistory: [],
  navIndex: -1,

  canGoBack: () => get().navIndex > 0,
  canGoForward: () => {
    const s = get()
    return s.navIndex < s.navHistory.length - 1
  },
  goBack: () => {
    const s = get()
    if (s.navIndex <= 0) return
    const newIndex = s.navIndex - 1
    const tabId = s.navHistory[newIndex]
    // Check the tab still exists
    const allTabs = [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
    if (!allTabs.find((t) => t.id === tabId)) return
    set({ activeTabId: tabId, navIndex: newIndex })
  },
  goForward: () => {
    const s = get()
    if (s.navIndex >= s.navHistory.length - 1) return
    const newIndex = s.navIndex + 1
    const tabId = s.navHistory[newIndex]
    const allTabs = [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
    if (!allTabs.find((t) => t.id === tabId)) return
    set({ activeTabId: tabId, navIndex: newIndex })
  },

  addTab: (cwd: string, options?: { title?: string; command?: string; args?: string[] }) => {
    // Route to the active group
    const activeGroup = get().activeGroupIndex
    if (activeGroup > 0) {
      return get().addTabToGroup(activeGroup, cwd, options)
    }

    tabCounter++
    const tabId = crypto.randomUUID()
    const paneGroupId = crypto.randomUUID()

    const paneGroup = makeTerminalPaneGroup(paneGroupId, cwd, options)

    // Use provided title, or derive from command name, or fallback to "Terminal N"
    const title = options?.title
      ?? (options?.command ? options.command.split('/').pop()?.split(' ')[0] ?? `Terminal ${tabCounter}` : `Terminal ${tabCounter}`)

    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: paneGroupId,
      paneGroups: new Map([[paneGroupId, paneGroup]])
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))

    // 0.38.0 Commit 4 — no cross-window broadcast here. The daemon's
    // `/cli/sessions/events` stream pushes `session_added` to every
    // connected window once the v2 spawn registers the PTY.

    return paneGroupId
  },

  removeTab: (tabId: string) => {
    // Never close the pinned system agent tab
    const tab = get().tabs.find((t) => t.id === tabId)
    if (tab?.isSystemAgent) return

    // #587 — closing a pinned HTML file tab means UNPIN, not just hide.
    // Route through unpinFileTab so the pin state stays consistent
    // (and so a re-pin later re-creates the tab rather than colliding).
    if (tab?.isPinnedFile) {
      const item = Array.from(tab.paneGroups.values())[0]?.items[0]
      if (item?.type === 'file-viewer') {
        get().unpinFileTab((item.data as FileViewerItemData).filePath)
        return
      }
    }

    // Kill all PTYs in the removed tab (PTYs survive tab switches for persistence)
    if (tab) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            closeTerminalForRenderer(data)
          }
        }
      }
    }

    set((state) => {
      const newTabs = state.tabs.filter((t) => t.id !== tabId)
      let newActiveId = state.activeTabId

      if (state.activeTabId === tabId) {
        const idx = state.tabs.findIndex((t) => t.id === tabId)
        if (newTabs.length > 0) {
          newActiveId = newTabs[Math.min(idx, newTabs.length - 1)].id
        } else {
          newActiveId = null
        }
      }

      return { tabs: newTabs, activeTabId: newActiveId }
    })

    // 0.38.0 Commit 4 — no cross-window broadcast here. The daemon's
    // `/cli/sessions/events` push delivers `session_removed` to other
    // windows after `closeTerminalForRenderer` unregisters the v2 PTY.
  },

  setActiveTab: (tabId: string) => {
    const state = get()
    if (tabId !== state.activeTabId) {
      // Push to nav history (truncate any forward history)
      const history = state.navHistory.slice(0, state.navIndex + 1)
      if (state.activeTabId) history.push(state.activeTabId)
      set({ activeTabId: tabId, navHistory: history, navIndex: history.length })
    } else {
      set({ activeTabId: tabId })
    }
    // per-client-view-state.md (Phase 2) — same primary-selection persist as
    // `setActiveTabInGroup`'s group-0 branch (this is the other group-0
    // selection entry point, e.g. ChatHistory's tab jump).
    persistGroup0Selection(state, tabId)
  },

  splitPane: (tabId, existingPaneGroupId, newPaneGroupId, newPane, direction) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const newPaneGroups = new Map(tab.paneGroups)
        // Create a new PaneGroup from the TerminalPaneData
        const pg: PaneGroup = {
          id: newPaneGroupId,
          items: [paneDataToItem(newPane)],
          activeItemIndex: 0,
        }
        // Override the terminal's terminalId to match paneGroupId for compat
        if (pg.items[0].type === 'terminal') {
          (pg.items[0].data as TerminalItemData).terminalId = newPaneGroupId
        }
        newPaneGroups.set(newPaneGroupId, pg)

        const newTree: MosaicNode<string> = {
          direction,
          first: existingPaneGroupId,
          second: newPaneGroupId,
          splitPercentage: 50
        }

        // Replace the existing paneGroup in the tree with the split
        const updatedTree = replaceInTree(tab.mosaicTree, existingPaneGroupId, newTree)

        return { ...tab, mosaicTree: updatedTree, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  updateMosaicTree: (tabId, tree) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, mosaicTree: tree }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  reorderTabs: (fromIndex, toIndex, groupIndex = 0) => {
    set((state) => {
      if (groupIndex === 0) {
        const tabs = [...state.tabs]
        // Don't reorder the pinned system agent tabs (always at the
        // front) or the pinned HTML file tabs (#587 — fixed slot right
        // after the system tabs). Either endpoint touching a pinned
        // tab cancels the reorder so the canonical
        // [system][pinned-file][regular] order is preserved.
        if (
          tabs[fromIndex]?.isSystemAgent || tabs[toIndex]?.isSystemAgent ||
          tabs[fromIndex]?.isPinnedFile || tabs[toIndex]?.isPinnedFile
        ) return {}
        const [moved] = tabs.splice(fromIndex, 1)
        tabs.splice(toIndex, 0, moved)
        return { tabs }
      }
      const extraGroups = state.extraGroups.map((g: { tabs: Tab[], activeTabId: string | null }, i: number) => {
        if (i !== groupIndex - 1) return g
        const tabs = [...g.tabs]
        const [moved] = tabs.splice(fromIndex, 1)
        tabs.splice(toIndex, 0, moved)
        return { ...g, tabs }
      })
      return { extraGroups }
    })
  },

  addPaneToTab: (tabId, paneGroupId, pane) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const newPaneGroups = new Map(tab.paneGroups)
        // Create a new PaneGroup containing the single item
        const pg: PaneGroup = {
          id: paneGroupId,
          items: [paneDataToItem(pane)],
          activeItemIndex: 0,
        }
        newPaneGroups.set(paneGroupId, pg)
        return { ...tab, paneGroups: newPaneGroups }
      })
    }))
  },

  removePaneFromTab: (tabId, paneGroupId) => {
    // Kill PTYs in the removed pane group
    const tab = get().tabs.find((t) => t.id === tabId)
    const pg = tab?.paneGroups.get(paneGroupId)
    if (pg) {
      for (const item of pg.items) {
        if (item.type === 'terminal') {
          const data = item.data as TerminalItemData
          closeTerminalForRenderer(data)
        }
      }
    }

    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.delete(paneGroupId)
        const newTree = removePaneFromTree(tab.mosaicTree, paneGroupId)
        return { ...tab, paneGroups: newPaneGroups, mosaicTree: newTree }
      })
    }))
  },

  moveItemBetweenPanes: (fromTabId, fromPaneGroupId, itemId, toTabId, toPaneGroupId) => {
    set((state) => {
      // Find the source item
      const allTabs = [...state.tabs, ...state.extraGroups.flatMap((g) => g.tabs)]
      const fromTab = allTabs.find((t) => t.id === fromTabId)
      const fromPg = fromTab?.paneGroups.get(fromPaneGroupId)
      if (!fromPg) return state

      const itemIdx = fromPg.items.findIndex((i) => i.id === itemId)
      if (itemIdx < 0) return state
      const item = fromPg.items[itemIdx]

      // Remove from source
      const newFromItems = fromPg.items.filter((_, i) => i !== itemIdx)
      const newFromActiveIdx = Math.min(fromPg.activeItemIndex, Math.max(0, newFromItems.length - 1))

      // Add to target
      const toTab = allTabs.find((t) => t.id === toTabId)
      const toPg = toTab?.paneGroups.get(toPaneGroupId)
      if (!toPg) return state

      const newToItems = [...toPg.items, item]

      const updateTab = (tab: Tab): Tab => {
        if (tab.id === fromTabId) {
          const newPg = new Map(tab.paneGroups)
          newPg.set(fromPaneGroupId, { ...fromPg, items: newFromItems, activeItemIndex: newFromActiveIdx })
          return { ...tab, paneGroups: newPg }
        }
        if (tab.id === toTabId) {
          const newPg = new Map(tab.paneGroups)
          newPg.set(toPaneGroupId, { ...toPg, items: newToItems, activeItemIndex: newToItems.length - 1 })
          return { ...tab, paneGroups: newPg }
        }
        return tab
      }

      return {
        tabs: state.tabs.map(updateTab),
        extraGroups: state.extraGroups.map((g) => ({
          ...g,
          tabs: g.tabs.map(updateTab),
        })),
      }
    })
  },

  getActiveTab: () => {
    const state = get()
    return state.tabs.find((t) => t.id === state.activeTabId)
  },

  openFileInPane: (tabId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        // Find the active (or first) paneGroup and add a file-viewer item to it
        const activePgId = getFirstLeaf(tab.mosaicTree)
        if (!activePgId) return tab

        const pg = tab.paneGroups.get(activePgId)
        if (!pg) return tab

        // Look for an existing unpinned file-viewer item in this paneGroup
        const unpinnedIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && !item.pinned
        )

        const newPaneGroups = new Map(tab.paneGroups)

        if (unpinnedIdx !== -1) {
          // Reuse the unpinned item — update its filePath
          const newItems = [...pg.items]
          newItems[unpinnedIdx] = {
            ...newItems[unpinnedIdx],
            data: { filePath },
          }
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: unpinnedIdx,
          })
        } else {
          // Add a new file-viewer item to the paneGroup
          const newItem: Item = {
            id: crypto.randomUUID(),
            type: 'file-viewer',
            data: { filePath },
            pinned: false,
          }
          const newItems = [...pg.items, newItem]
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: newItems.length - 1,
          })
        }

        return { ...tab, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  stampAgentSessionId: (agentName: string, projectPath: string, sessionId: string, ownerProjectId: string) => {
    if (!sessionId) return
    // GH#608: only stamp when the calling chat pane OWNS the tabs currently
    // in `state.tabs`. Matching on (agentName, projectPath) alone let a
    // session from one workspace land on a DIFFERENT workspace's pinned
    // chat item whenever the two shared an agentName + projectPath —
    // restoring the wrong chat history.
    //
    // GH#679 regression fix: the original guard compared `ownerProjectId`
    // against the PROJECTS store's `currentActiveProjectId()`. That reads a
    // SEPARATE store that can lag the tab set during host-switch /
    // projects re-fetch / multi-window flows, so a legitimate
    // same-workspace stamp got silently dropped and the chat-history
    // dropdown looked dead (the DB session updated but the live PTY never
    // swapped). The authoritative "whose tabs are these" identity lives
    // HERE in tabs.ts: `activeWorkspaceKey` ("projectId:workspaceId") is
    // set in lockstep with the tabs loaded by loadLayoutForWorkspace /
    // restoreWorkspace — no cross-store race. Compare against its project
    // portion. (Falls back to the projects-store getter only when
    // activeWorkspaceKey is unset, e.g. a vitest unit that never restores a
    // workspace.) When activeProjectId can't be resolved at all we let the
    // stamp through — `state.tabs` is the active workspace's by construction.
    const activeWorkspaceKey = get().activeWorkspaceKey
    const activeProjectId = activeWorkspaceKey
      ? activeWorkspaceKey.split(':')[0]
      : currentActiveProjectId()
    if (ownerProjectId && activeProjectId && ownerProjectId !== activeProjectId) return
    let mutated = false
    set((state) => {
      const next = state.tabs.map((tab) => {
        let tabMutated = false
        const nextPaneGroups = new Map(tab.paneGroups)
        for (const [pgId, pg] of tab.paneGroups) {
          let pgMutated = false
          const nextItems = pg.items.map((item) => {
            if (item.type !== 'agent') return item
            const d = item.data as AgentItemData
            if (d.agentName !== agentName || d.projectPath !== projectPath) return item
            // 0.37.12 P1C — only stamp the CHAT pinned tab. Inbox
            // tabs share agentName + projectPath but don't render
            // Claude, so sessionId on those is unused garbage. Also
            // future-proofs against worktree-chat agent items
            // (which use a different agentName but still match
            // projectPath in their parent workspace context).
            if (d.section !== 'chat') return item
            if (d.sessionId === sessionId) return item
            pgMutated = true
            return { ...item, data: { ...d, sessionId } }
          })
          if (pgMutated) {
            tabMutated = true
            nextPaneGroups.set(pgId, { ...pg, items: nextItems })
          }
        }
        if (!tabMutated) return tab
        mutated = true
        return { ...tab, paneGroups: nextPaneGroups }
      })
      return mutated ? { tabs: next } : state
    })
    // Trigger a layout save so the new sessionId lands in DB on the
    // next debounce. persistActiveWorkspace already runs on a tab
    // mutation — calling explicitly here is belt-and-suspenders for
    // the case where the renderer is mid-cleanup at app quit.
    if (mutated) get().persistActiveWorkspace()
  },

  openAgentPane: (agentName: string, projectPath: string, title?: string) => {
    const state = get()

    // If this is the primary agent (manager lead, k2so agent, or custom agent),
    // redirect to the pinned system agent tab if it exists
    // If a pinned system agent tab exists, redirect to it for the primary agent
    const sysTab = state.tabs.find((t) => t.isSystemAgent)
    if (sysTab) {
      // Check if this is the agent in the system tab, or a known primary name
      const sysAgentName = Array.from(sysTab.paneGroups.values())[0]?.items[0]?.type === 'agent'
        ? (Array.from(sysTab.paneGroups.values())[0].items[0].data as AgentItemData).agentName
        : null
      // 0.39.0f Phase 2.1: the `__lead__` literal was removed from
      // routing — the system tab redirect now keys solely on the
      // resolved primary agent name or the workspace-board sentinel.
      if (agentName === sysAgentName || agentName === '__workspace__') {
        set({ activeTabId: sysTab.id })
        return
      }
    }

    // Check if a tab for this agent already exists — switch to it
    for (const tab of state.tabs) {
      for (const [, pg] of tab.paneGroups) {
        const match = pg.items.find(
          (item) => item.type === 'agent' && (item.data as AgentItemData).agentName === agentName
        )
        if (match) {
          set({ activeTabId: tab.id })
          return
        }
      }
    }

    // Create a new tab with a single agent pane
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const agentItem: Item = {
      id: crypto.randomUUID(),
      type: 'agent',
      data: { agentName, projectPath },
    }
    const pg: PaneGroup = {
      id: pgId,
      items: [agentItem],
      activeItemIndex: 0,
    }
    const tabTitle = title ?? (agentName === '__workspace__' ? 'Work Board' : agentName)
    const tab: Tab = {
      id: tabId,
      title: tabTitle,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]]),
    }
    set((s) => ({ tabs: [...s.tabs, tab], activeTabId: tabId }))
  },

  openHeartbeatTab: async (
    projectPath: string,
    heartbeatName: string,
    _options?: { existingTerminalId?: string },
  ): Promise<string | null> => {
    // Phase 4 of the heartbeat-active-session-tracking PRD: ask the
    // daemon for the heartbeat's current live PTY. If one exists,
    // flip the `surfaced` flag → daemon emits `session:surfaced`
    // → renderer's listener attaches a tab to the existing PTY (no
    // duplicate `claude --resume`, single canonical session). If no
    // live PTY (column was null OR pointed at a corpse), fall through
    // to the legacy fresh-resume path.
    let active: {
      name: string
      claudeSessionId: string | null
      activeTerminalId: string | null
      activeAgentName: string | null
      sessionAlive: boolean
      isV2: boolean
    } | null = null
    try {
      const raw = await invoke<string>('k2so_heartbeat_active_session', {
        projectPath,
        name: heartbeatName,
      })
      active = JSON.parse(raw)
    } catch (err) {
      console.warn('[openHeartbeatTab] active-session lookup failed:', err)
    }

    if (active?.sessionAlive && active.activeTerminalId) {
      // Live PTY exists — surface it. The daemon's listener for the
      // surfaced flag emits `session:surfaced` which triggers
      // `createCompanionTab` in active-agents.ts to attach a tab
      // (with the heartbeat-named title) to the existing terminal.
      // First check whether the renderer already has a tab for this
      // session — if so, just focus it instead of round-tripping
      // through SessionSurfaced and creating a duplicate.
      //
      // Match key reasoning: the daemon's `active_terminal_id` is the
      // session.session_id (e.g. `009bcb4c…`), NOT the renderer's
      // terminalId. The bridge is the agent_name (`tab-<rendererId>`
      // for tabs the renderer spawned). Strip the `tab-` prefix to
      // get the renderer-side id and match against that. For
      // daemon-spawned sessions registered under a non-`tab-` name
      // (e.g. just the workspace agent name), there's no renderer
      // tab yet — fall through to the surface path.
      const state = get()
      const rendererId = active.activeAgentName?.startsWith('tab-')
        ? active.activeAgentName.slice('tab-'.length)
        : null
      if (rendererId) {
        for (const tab of state.tabs) {
          for (const [, pg] of tab.paneGroups) {
            for (const item of pg.items) {
              if (item.type !== 'terminal') continue
              const td = item.data as TerminalItemData
              if (td.terminalId === rendererId) {
                set({ activeTabId: tab.id })
                return tab.id
              }
            }
          }
        }
      }
      // Not yet a tab — flip surfaced. Resolve the agent name first;
      // for daemon-spawned heartbeat sessions the agent name on the
      // agent_sessions row is the workspace's primary agent (the one
      // with type custom / manager / k2so). Fall back to listing if
      // needed.
      let agentName: string | null = null
      try {
        const agents = await invoke<Array<{
          name: string
          agentType: string
        }>>('k2so_agents_list', { projectPath }).catch(() => [])
        agentName = agents.find((a) =>
          a.agentType === 'custom' || a.agentType === 'manager' || a.agentType === 'k2so'
        )?.name ?? agents[0]?.name ?? null
      } catch { /* fall back to spawn-fresh path */ }
      if (agentName) {
        try {
          const presetArgs = await resolveClaudePresetArgs()
          const args = [...presetArgs, '--resume', active.claudeSessionId ?? '']
          await daemonCliPost('session/set-surfaced', {
            project_path: projectPath,
            agent_name: agentName,
            surfaced: true,
            terminal_id: active.activeTerminalId,
            command: 'claude',
            args,
            heartbeat_name: heartbeatName,
            // The daemon's v2_session_map key for the existing PTY.
            // TerminalPane uses this to attach via /cli/sessions/v2/spawn
            // instead of spawning a fresh resume — see the PRD.
            attach_agent_name: active.activeAgentName,
          })
          // The session:surfaced listener will create the tab
          // asynchronously; don't return a tab id yet.
          return null
        } catch (err) {
          console.warn('[openHeartbeatTab] surfaced flag flip failed; spawning fresh:', err)
        }
      }
    }

    // No live PTY (or the surface path bailed) — legacy fresh-resume.
    const list = await invoke<Array<{
      name: string
      lastSessionId: string | null
      enabled: boolean
    }>>('k2so_heartbeat_list', { projectPath }).catch(() => [])
    const hb = list.find((h) => h.name === heartbeatName)
    if (!hb) {
      console.warn('[openHeartbeatTab] heartbeat not found:', heartbeatName)
      return null
    }
    if (!hb.lastSessionId) {
      // Scheduled (no fire yet). Caller should fire it first; we
      // don't auto-launch here because it'd surprise the user (the
      // Launch button is the explicit fire action).
      console.info('[openHeartbeatTab] heartbeat has no saved session yet; click Launch first')
      return null
    }

    // Verify the JSONL exists on disk before passing `--resume`.
    // wake_headless saves last_session_id synchronously at spawn time,
    // but claude doesn't write the JSONL until it's processed input.
    // A daemon restart during that window leaves a "ghost" session id
    // pointing at a missing file — `claude --resume <ghost>` would
    // fail with "No conversation found" and lock the user into a
    // broken tab on every retry. The next heartbeat fire will
    // self-heal via smart_launch's matching JSONL check, so we just
    // surface the wait state to the user here.
    const jsonlExists = await daemonCliGet<{ exists: boolean }>('chat/session-exists', {
      project_path: projectPath,
      session_id: hb.lastSessionId,
    })
      .then((r) => r.exists)
      .catch(() => true)
    if (!jsonlExists) {
      console.info(
        '[openHeartbeatTab] saved session %s has no JSONL on disk yet; click Launch to refire',
        hb.lastSessionId,
      )
      return null
    }

    // Read the user's claude preset args (e.g. --dangerously-skip-permissions)
    // so the resumed session has the same permissions as a fresh launch.
    const presetArgs = await resolveClaudePresetArgs()
    const args = [...presetArgs, '--resume', hb.lastSessionId]
    const title = `${heartbeatName} (heartbeat)`

    // Focus an existing tab that's already running this resume if
    // one is open — match by command + sessionId in args, the same
    // hook ChatHistory's tab-search uses for chat resumes.
    {
      const state = get()
      for (const tab of state.tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type !== 'terminal') continue
            const td = item.data as TerminalItemData
            if (td.command !== 'claude') continue
            if (td.args?.includes(hb.lastSessionId)) {
              set({ activeTabId: tab.id })
              return tab.id
            }
          }
        }
      }
    }

    // Open a new tab. addTabToGroup handles the rest — auto-generates
    // a terminal id, registers in the active group, broadcasts the
    // tab-add event, etc.
    const targetGroup = get().splitCount > 1 ? get().splitCount - 1 : 0
    const tabId = get().addTabToGroup(targetGroup, projectPath, { title, command: 'claude', args })

    // Force v2 renderer for heartbeat tabs regardless of the user's
    // workspace renderer choice — heartbeat-spawned PTYs already only
    // live in v2_session_map, so a tab attached to that session must
    // use the v2 path. See `.k2so/prds/heartbeat-active-session-tracking.md`.
    if (tabId) {
      set((s) => ({
        tabs: s.tabs.map((t) => {
          if (t.id !== tabId) return t
          const updatedGroups = new Map(t.paneGroups)
          for (const [pgId, pg] of updatedGroups) {
            const updatedItems = pg.items.map((item) => {
              if (item.type !== 'terminal') return item
              return {
                ...item,
                data: { ...item.data, renderer: 'alacritty-v2' as const },
              }
            })
            updatedGroups.set(pgId, { ...pg, items: updatedItems })
          }
          return { ...t, paneGroups: updatedGroups }
        }),
      }))
    }
    return tabId
  },

  ensureSystemAgentTabs: (agentName: string, projectPath: string, _title: string) => {
    const state = get()

    // The split landed in 0.36.0: each agent gets up to two pinned tabs.
    // Canonical order is Chat first, Inbox second — fixed regardless of
    // creation history. Workspace-board mode (`__workspace__`) is the
    // exception — it has no chat surface, just the kanban — so we
    // create only the Inbox tab in that case.
    const isWorkspaceBoard = agentName === '__workspace__'
    const wantSections: Array<'inbox' | 'chat'> = isWorkspaceBoard
      ? ['inbox']
      : ['chat', 'inbox']

    // Index existing system tabs by section so we can keep their state
    // (active item, pane group ids) when re-ordering.
    const existingTabs = state.tabs.filter((t) => t.isSystemAgent)
    const nonSystemTabs = state.tabs.filter((t) => !t.isSystemAgent)
    const bySection = new Map<string, Tab>()
    for (const t of existingTabs) {
      const item = Array.from(t.paneGroups.values())[0]?.items[0]
      if (item?.type === 'agent') {
        const d = item.data as AgentItemData
        bySection.set(d.section ?? 'inbox', t)
      }
    }

    // #658 — sessionId hint for a freshly-created Chat tab. When the
    // saved layout hasn't restored yet (cold-boot race) and we have to
    // create the Chat tab from scratch, seed it with the chat session id
    // persisted in this workspace's saved layout (in-memory cache loaded
    // by `loadWorkspaceSessionsFromDb`). Carrying the sessionId lets
    // AgentChatPane take the fast `--resume` path instead of the slower
    // `resumeChatArgs` daemon round-trip. Only valid for THIS workspace's
    // own saved layout, and only when the saved agent item's projectPath
    // matches the workspace we're ensuring (a path mismatch means the
    // session belonged to a different workspace — drop it, same rule as
    // reconcileSystemAgentTab).
    const savedChatSessionId = ((): string | undefined => {
      const activeKey = state.activeWorkspaceKey
      if (!activeKey) return undefined
      const layout = state.workspaceLayouts[activeKey]
      if (!layout?.tabs) return undefined
      for (const t of layout.tabs) {
        const groups = t.paneGroups
        if (!groups) continue
        for (const pg of Object.values(groups)) {
          for (const si of pg.items ?? []) {
            if (si.type === 'agent' && (si.section ?? 'inbox') === 'chat' && si.sessionId) {
              if ((si.projectPath ?? projectPath) === projectPath) return si.sessionId
            }
          }
        }
      }
      return undefined
    })()

    // Build the canonical-ordered system tab list, creating any
    // missing sections as we go. Idempotent: existing tabs are
    // preserved with all their state; only the ordering changes
    // when a previous insertion landed sections out of order.
    const orderedSystemTabs: Tab[] = []
    let firstTabId: string | null = null
    // #658 — id of the Chat tab in the ordered list, so we can adopt it
    // as the active tab below when nothing else is currently active.
    let chatTabId: string | null = null
    for (const section of wantSections) {
      const existing = bySection.get(section)
      if (existing) {
        // Heal stale workspace context on the pinned tab. A system agent
        // tab restored from this workspace's saved layout can carry an
        // agentName/projectPath from a *different* workspace — the
        // serialized layout stores projectPath (see serializeTab) and
        // restoreLayout replays it verbatim, so a value baked in under
        // another workspace survives the switch. The pinned tab IS the
        // current workspace's own agent surface, so reconcile its agent
        // item(s) to the authoritative agentName/projectPath we were
        // called with (sourced from project.path via
        // ensurePinnedAgentTabForMode). Without this, the Chat/Inbox tab
        // keeps routing to the wrong workspace even though new terminal
        // tabs — which spawn against the live workspace cwd — are correct.
        // When projectPath changes, drop the chat sessionId: that Claude
        // session belonged to the old workspace.
        const reconciled = reconcileSystemAgentTab(existing, agentName, projectPath)
        orderedSystemTabs.push(reconciled)
        if (!firstTabId) firstTabId = reconciled.id
        if (section === 'chat') chatTabId = reconciled.id
        continue
      }
      const tabId = crypto.randomUUID()
      if (!firstTabId) firstTabId = tabId
      if (section === 'chat') chatTabId = tabId
      const pgId = crypto.randomUUID()
      const agentItem: Item = {
        id: crypto.randomUUID(),
        type: 'agent',
        // #658 — seed the freshly-created Chat tab with the saved chat
        // sessionId (when we have one) so AgentChatPane resumes the same
        // Claude session via the fast `--resume` path. Inbox/board tabs
        // don't render Claude, so they never carry a sessionId.
        data: section === 'chat' && savedChatSessionId
          ? { agentName, projectPath, section, sessionId: savedChatSessionId }
          : { agentName, projectPath, section },
      }
      const pg: PaneGroup = {
        id: pgId,
        items: [agentItem],
        activeItemIndex: 0,
      }
      // Chat tab title is always "Chat". Inbox uses the workspace-board
      // label when in board mode, otherwise the section name.
      const tabTitle = section === 'chat'
        ? 'Chat'
        : isWorkspaceBoard ? 'Work Board' : 'Inbox'
      orderedSystemTabs.push({
        id: tabId,
        title: tabTitle,
        mosaicTree: pgId,
        paneGroups: new Map([[pgId, pg]]),
        isSystemAgent: true,
      })
    }

    // #658 — active-tab adoption. When this ensure is what surfaces the
    // pinned Chat tab for the workspace being activated and there is no
    // valid active tab yet (cold boot before restore set one, or the
    // current activeTabId belongs to a different workspace's tab that's
    // no longer present), make the Chat tab active. Without this the
    // pinned tab exists but isn't active → `isTabVisible` is false → its
    // grid-WS never opens → the session never spawns until a manual
    // refresh. We only adopt when nothing else is currently active; if
    // restoreLayout already set a live activeTabId (the normal switch
    // path) we leave the user's active tab untouched.
    const nextTabs = [...orderedSystemTabs, ...nonSystemTabs]
    const currentActiveId = state.activeTabId
    const activeStillPresent = currentActiveId != null
      && nextTabs.some((t) => t.id === currentActiveId)
    const shouldAdoptChat = !activeStillPresent && chatTabId != null
    if (shouldAdoptChat) {
      set({ tabs: nextTabs, activeTabId: chatTabId })
    } else {
      // Always write the canonical order back. Cheap when nothing
      // changed (Zustand's shallow eq sees a new array but the items
      // are the same references), and guarantees the strip looks
      // right after a back-fill or a half-migrated layout restore.
      set({ tabs: nextTabs })
    }
    return firstTabId ?? orderedSystemTabs[0]?.id ?? ''
  },

  removeSystemAgentTab: () => {
    set((s) => ({ tabs: s.tabs.filter((t) => !t.isSystemAgent) }))
  },

  activateSystemAgentTab: () => {
    const state = get()
    const sysTab = state.tabs.find((t) => t.isSystemAgent)
    if (sysTab) {
      set({ activeTabId: sysTab.id })
    }
  },

  getSystemAgentTab: () => {
    return get().tabs.find((t) => t.isSystemAgent)
  },

  // ── Pinned HTML file tabs (#587) ─────────────────────────────────

  pinFileAsTab: (filePath: string) => {
    const filePathOf = (tab: Tab): string | null => {
      const item = Array.from(tab.paneGroups.values())[0]?.items[0]
      if (item?.type !== 'file-viewer') return null
      return (item.data as FileViewerItemData).filePath
    }

    set((state) => {
      // Already pinned? Just focus it (idempotent — no duplicate tab).
      const existing = state.tabs.find((t) => t.isPinnedFile && filePathOf(t) === filePath)
      if (existing) {
        return { activeTabId: existing.id }
      }

      // Build a dedicated pinned-file tab. The file-viewer item is
      // `pinned: true` so the pane's own pin affordance reflects the
      // pinned state and `openFileInPane` won't recycle this slot.
      const pgId = crypto.randomUUID()
      const tabId = crypto.randomUUID()
      const title = filePath.split('/').pop() || filePath
      const pg = makeFileViewerPaneGroup(pgId, filePath, true)
      const newTab: Tab = {
        id: tabId,
        title,
        mosaicTree: pgId,
        paneGroups: new Map([[pgId, pg]]),
        isPinnedFile: true,
      }

      // Order: [system…] [pinned-file…] [regular…]. Insert the new
      // pinned tab after the existing system + pinned-file tabs but
      // before the first regular tab — mirrors how ensureSystemAgentTabs
      // keeps system tabs at the front.
      const leading: Tab[] = []
      const regular: Tab[] = []
      for (const t of state.tabs) {
        if (t.isSystemAgent || t.isPinnedFile) leading.push(t)
        else regular.push(t)
      }
      return {
        tabs: [...leading, newTab, ...regular],
        activeTabId: tabId,
      }
    })
    get().persistActiveWorkspace()
  },

  unpinFileTab: (filePath: string) => {
    const filePathOf = (tab: Tab): string | null => {
      const item = Array.from(tab.paneGroups.values())[0]?.items[0]
      if (item?.type !== 'file-viewer') return null
      return (item.data as FileViewerItemData).filePath
    }

    set((state) => {
      const target = state.tabs.find((t) => t.isPinnedFile && filePathOf(t) === filePath)
      if (!target) return state
      const newTabs = state.tabs.filter((t) => t.id !== target.id)
      let newActiveId = state.activeTabId
      if (state.activeTabId === target.id) {
        const idx = state.tabs.findIndex((t) => t.id === target.id)
        newActiveId = newTabs.length > 0
          ? newTabs[Math.min(idx, newTabs.length - 1)].id
          : null
      }
      return { tabs: newTabs, activeTabId: newActiveId }
    })
    get().persistActiveWorkspace()
  },

  isFilePinned: (filePath: string): boolean => {
    return get().tabs.some((t) => {
      if (!t.isPinnedFile) return false
      const item = Array.from(t.paneGroups.values())[0]?.items[0]
      if (item?.type !== 'file-viewer') return false
      return (item.data as FileViewerItemData).filePath === filePath
    })
  },

  openFileAsTab: (filePath: string) => {
    const state = get()

    // Locate a matching file-viewer item within a tab's pane groups.
    const findFileInTab = (tab: Tab): { pgId: string; itemIdx: number } | null => {
      for (const [pgId, pg] of tab.paneGroups) {
        const itemIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && (item.data as FileViewerItemData).filePath === filePath
        )
        if (itemIdx !== -1) return { pgId, itemIdx }
      }
      return null
    }

    // Activate the matching item inside its pane group (no-op if already active).
    const activateMatch = (tab: Tab, pgId: string, itemIdx: number): Tab => {
      const pg = tab.paneGroups.get(pgId)
      if (!pg || pg.activeItemIndex === itemIdx) return tab
      const newPaneGroups = new Map(tab.paneGroups)
      newPaneGroups.set(pgId, { ...pg, activeItemIndex: itemIdx })
      return { ...tab, paneGroups: newPaneGroups }
    }

    // Search the main tab group first.
    for (const tab of state.tabs) {
      const found = findFileInTab(tab)
      if (found) {
        const newTabs = state.tabs.map((t) =>
          t.id === tab.id ? activateMatch(t, found.pgId, found.itemIdx) : t
        )
        set({ tabs: newTabs, activeTabId: tab.id, activeGroupIndex: 0 })
        return
      }
    }

    // Search split-column groups so a file already open in another column
    // gets focused instead of duplicated.
    for (let gi = 0; gi < state.extraGroups.length; gi++) {
      const group = state.extraGroups[gi]
      for (const tab of group.tabs) {
        const found = findFileInTab(tab)
        if (found) {
          const newGroups = [...state.extraGroups]
          newGroups[gi] = {
            ...group,
            activeTabId: tab.id,
            tabs: group.tabs.map((t) =>
              t.id === tab.id ? activateMatch(t, found.pgId, found.itemIdx) : t
            ),
          }
          set({ extraGroups: newGroups, activeGroupIndex: gi + 1 })
          return
        }
      }
    }

    // No existing tab — create a new one in the main group.
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const title = filePath.split('/').pop() || filePath
    const pg = makeFileViewerPaneGroup(pgId, filePath, false)
    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]]),
    }
    set((s) => ({ tabs: [...s.tabs, tab], activeTabId: tabId }))
  },

  openFileInPaneGroup: (tabId: string, paneGroupId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        const unpinnedIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && !item.pinned
        )

        const newPaneGroups = new Map(tab.paneGroups)

        if (unpinnedIdx !== -1) {
          const newItems = [...pg.items]
          newItems[unpinnedIdx] = { ...newItems[unpinnedIdx], data: { filePath } }
          newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: unpinnedIdx })
        } else {
          const newItem: Item = {
            id: crypto.randomUUID(),
            type: 'file-viewer',
            data: { filePath },
            pinned: false,
          }
          const newItems = [...pg.items, newItem]
          newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: newItems.length - 1 })
        }

        return { ...tab, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  openDiffInPane: (tabId: string, filePath: string) => {
    set((state) => {
      const tabs = state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab

        const activePgId = getFirstLeaf(tab.mosaicTree)
        if (!activePgId) return tab

        const pg = tab.paneGroups.get(activePgId)
        if (!pg) return tab

        // Look for an existing unpinned file-viewer item to reuse
        const unpinnedIdx = pg.items.findIndex(
          (item) => item.type === 'file-viewer' && !item.pinned
        )

        const newPaneGroups = new Map(tab.paneGroups)
        const diffData: FileViewerItemData = { filePath, mode: 'diff' }

        if (unpinnedIdx !== -1) {
          const newItems = [...pg.items]
          newItems[unpinnedIdx] = {
            ...newItems[unpinnedIdx],
            data: diffData,
          }
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: unpinnedIdx,
          })
        } else {
          const newItem: Item = {
            id: crypto.randomUUID(),
            type: 'file-viewer',
            data: diffData,
            pinned: false,
          }
          const newItems = [...pg.items, newItem]
          newPaneGroups.set(activePgId, {
            ...pg,
            items: newItems,
            activeItemIndex: newItems.length - 1,
          })
        }

        return { ...tab, paneGroups: newPaneGroups }
      })

      return { tabs }
    })
  },

  pinPane: (tabId: string, paneGroupId: string) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        // Pin the active item if it's a file-viewer
        const activeItem = pg.items[pg.activeItemIndex]
        if (!activeItem || activeItem.type !== 'file-viewer') return tab

        const newItems = [...pg.items]
        newItems[pg.activeItemIndex] = { ...activeItem, pinned: true }
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems })
        return { ...tab, paneGroups: newPaneGroups }
      })
    }))
  },

  unpinPane: (tabId: string, paneGroupId: string) => {
    set((state) => ({
      tabs: state.tabs.map((tab) => {
        if (tab.id !== tabId) return tab
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        const activeItem = pg.items[pg.activeItemIndex]
        if (!activeItem || activeItem.type !== 'file-viewer') return tab

        const newItems = [...pg.items]
        newItems[pg.activeItemIndex] = { ...activeItem, pinned: false }
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems })
        return { ...tab, paneGroups: newPaneGroups }
      })
    }))
  },

  openFileInNewTab: (filePath: string) => {
    const paneGroupId = crypto.randomUUID()
    const tabId = crypto.randomUUID()
    const fileName = filePath.split('/').pop() || 'File'

    const pg = makeFileViewerPaneGroup(paneGroupId, filePath, true)

    const tab: Tab = {
      id: tabId,
      title: fileName,
      mosaicTree: paneGroupId,
      paneGroups: new Map([[paneGroupId, pg]])
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))
  },

  openUntitledDocument: (cwd: string) => {
    // Count existing untitled docs to generate a unique name
    const state = get()
    const allTabs = [
      ...state.tabs,
      ...state.extraGroups.flatMap((g: { tabs: Tab[] }) => g.tabs)
    ]
    const untitledPattern = /^Untitled(?:-(\d+))?$/
    let maxNum = 0
    for (const t of allTabs) {
      const m = untitledPattern.exec(t.title)
      if (m) maxNum = Math.max(maxNum, m[1] ? parseInt(m[1], 10) : 1)
    }
    const num = maxNum + 1
    const title = num === 1 ? 'Untitled' : `Untitled-${num}`

    const paneGroupId = crypto.randomUUID()
    const tabId = crypto.randomUUID()
    const filePath = `${cwd}/${title}`

    const pg = makeFileViewerPaneGroup(paneGroupId, filePath, true)

    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: paneGroupId,
      paneGroups: new Map([[paneGroupId, pg]]),
      isDirty: true, // Mark as dirty since it's unsaved
    }

    set((state) => ({
      tabs: [...state.tabs, tab],
      activeTabId: tabId
    }))
  },

  setTabTitle: (tabId: string, title: string) => {
    // 0.37.4 Phase B: pinned system agent tabs (Chat / Inbox) own
    // their bar titles — they're picked deliberately by
    // `ensureSystemAgentTabs` ("Chat", "Inbox", "Work Board") and
    // refer to the UI surface, not the underlying session.
    // Daemon-pushed `LabelChanged` events for the canonical
    // session still update the in-pane header (AgentChatPane reads
    // from `useSessionLabel` / `display_name` directly), but the
    // tab BAR keeps its functional label so the pinned tabs read
    // consistently across workspaces.
    const allTabs = (() => {
      const s = get()
      return [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
    })()
    const target = allTabs.find((t) => t.id === tabId)
    if (target?.isSystemAgent) {
      return
    }
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, title }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
    // 0.39.39 (#676) — tab titles are now daemon-canonical so a rename in
    // one client/window converges on every connected client + the mobile
    // companion. Persist to the daemon-owned `tab_titles` store (which
    // broadcasts `TabTitleChanged`); the local-layout persistence (via the
    // auto-save debounce) stays as the fallback when the daemon doesn't
    // advertise the broadcast capability. The store is keyed by
    // (projectId, tabId), so we send the active workspace's projectId.
    if (serverSupports('daemon-broadcasts')) {
      const projectId = get().activeWorkspaceKey?.split(':')[0]
      if (projectId) {
        daemonCliPost('workspace/set-tab-title', { projectId, tabId, title }).catch((err) => {
          console.error('[tabs] set-tab-title persist failed:', err)
        })
      }
    }
  },

  applyDaemonTabTitle: (tabId: string, title: string) => {
    // Local-only apply (no re-POST) for the broadcast handler + on-load
    // snapshot. Same pinned-system-agent guard as setTabTitle: those tabs
    // own their functional bar labels and ignore canonical renames.
    const allTabs = (() => {
      const s = get()
      return [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
    })()
    const target = allTabs.find((t) => t.id === tabId)
    if (!target || target.isSystemAgent) return
    if (target.title === title) return
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, title }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  renameTabByTitle: (oldTitle: string, newTitle: string) => {
    set((state) => {
      const updateTab = (tab: any) =>
        tab.title === oldTitle ? { ...tab, title: newTitle } : tab
      return {
        tabs: state.tabs.map(updateTab),
        extraGroups: state.extraGroups.map((group: any) => ({
          ...group,
          tabs: group.tabs.map(updateTab),
        })),
      }
    })
  },

  setTabDirty: (tabId: string, dirty: boolean) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, isDirty: dirty }))
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  setFileViewerState: (tabId: string, paneId: string, itemId: string, viewerState: { scrollTop?: number; cursorPos?: number }) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneId)
        if (!pg) return tab
        const newItems = pg.items.map((item) => {
          if (item.id !== itemId || item.type !== 'file-viewer') return item
          const prevData = item.data as FileViewerItemData
          return { ...item, data: { ...prevData, ...viewerState } }
        })
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneId, { ...pg, items: newItems })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  /** @deprecated Use openFileInPane instead */
  openMarkdownPane: (tabId: string, filePath: string, _splitDirection: 'row' | 'column' = 'row') => {
    get().openFileInPane(tabId, filePath)
  },

  // ── NEW: PaneGroup item management ────────────────────────────────────

  addItemToPaneGroup: (tabId: string, paneGroupId: string, item: Item) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab
        const newItems = [...pg.items, item]
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: newItems.length - 1 })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  activateItemInPaneGroup: (tabId: string, paneGroupId: string, itemIndex: number) => {
    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg || itemIndex < 0 || itemIndex >= pg.items.length) return tab
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, activeItemIndex: itemIndex })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  closeItemInPaneGroup: (tabId: string, paneGroupId: string, itemId: string) => {
    // Kill the PTY for the removed terminal item
    const tab = findTabAcrossGroups(get(), tabId)
    const pg = tab?.paneGroups.get(paneGroupId)
    const removedItem = pg?.items.find((item) => item.id === itemId)
    if (removedItem?.type === 'terminal') {
      const data = removedItem.data as TerminalItemData
      closeTerminalForRenderer(data)
    }

    set((state) => {
      const result = mapTabAcrossGroups(state, tabId, (tab) => {
        const pg = tab.paneGroups.get(paneGroupId)
        if (!pg) return tab

        const newItems = pg.items.filter((item) => item.id !== itemId)

        if (newItems.length === 0) {
          const newPaneGroups = new Map(tab.paneGroups)
          newPaneGroups.delete(paneGroupId)
          const newTree = removePaneFromTree(tab.mosaicTree, paneGroupId)
          return { ...tab, paneGroups: newPaneGroups, mosaicTree: newTree }
        }

        const newActiveIndex = Math.min(pg.activeItemIndex, newItems.length - 1)
        const newPaneGroups = new Map(tab.paneGroups)
        newPaneGroups.set(paneGroupId, { ...pg, items: newItems, activeItemIndex: newActiveIndex })
        return { ...tab, paneGroups: newPaneGroups }
      })
      return { tabs: result.tabs, extraGroups: result.extraGroups }
    })
  },

  getActivePaneGroupId: (tabId: string): string | null => {
    const state = get()
    const tab = findTabAcrossGroups(state, tabId)
    if (!tab) return null
    return getFirstLeaf(tab.mosaicTree)
  },

  splitActivePane: (cwd: string, maxPanes: number = 3): boolean => {
    const state = get()
    const tab = state.tabs.find((t) => t.id === state.activeTabId)
    if (!tab) return false

    const currentCount = countLeaves(tab.mosaicTree)
    if (currentCount >= maxPanes) return false

    const existingPaneId = getFirstLeaf(tab.mosaicTree)
    if (!existingPaneId) return false

    const newPaneId = crypto.randomUUID()
    const newPane: TerminalPaneData = {
      type: 'terminal',
      terminalId: newPaneId,
      cwd,
    }

    get().splitPane(tab.id, existingPaneId, newPaneId, newPane, 'row')
    return true
  },

  getActivePaneCount: (): number => {
    const state = get()
    const tab = state.tabs.find((t) => t.id === state.activeTabId)
    if (!tab) return 0
    return countLeaves(tab.mosaicTree)
  },

  // ── Tab Groups ──────────────────────────────────────────────────────

  splitTerminalArea: (cwd: string) => {
    const state = get()
    if (state.splitCount >= 3) return

    // Create a new group with a fresh terminal tab
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const pg = makeTerminalPaneGroup(pgId, cwd)
    const tab: Tab = {
      id: tabId,
      title: `Terminal ${tabCounter}`,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]])
    }

    const newGroups = [...state.extraGroups, { tabs: [tab], activeTabId: tabId }]
    set({
      splitCount: state.splitCount + 1,
      extraGroups: newGroups,
      activeGroupIndex: state.splitCount  // focus the new group
    })
  },

  unsplitTerminalArea: () => {
    const state = get()
    if (state.splitCount <= 1) return

    // Move tabs from the rightmost group into the group to its left
    // (don't kill PTYs — preserve all terminals)
    const removedGroup = state.extraGroups[state.extraGroups.length - 1]
    const removedTabs = removedGroup?.tabs ?? []

    if (state.splitCount === 2) {
      // Removing group 1 → merge its tabs into group 0
      set({
        tabs: [...state.tabs, ...removedTabs],
        splitCount: 1,
        extraGroups: [],
        activeGroupIndex: 0
      })
    } else {
      // Removing group 2 → merge its tabs into group 1
      const newGroups = [...state.extraGroups]
      const targetGroup = newGroups[0] // group 1 (index 0 in extraGroups)
      newGroups[0] = {
        tabs: [...targetGroup.tabs, ...removedTabs],
        activeTabId: targetGroup.activeTabId
      }
      newGroups.pop() // remove the last group
      set({
        splitCount: state.splitCount - 1,
        extraGroups: newGroups,
        activeGroupIndex: Math.min(state.activeGroupIndex, state.splitCount - 2)
      })
    }
  },

  setActiveGroup: (index: number) => {
    set({ activeGroupIndex: index })
  },

  addTabToGroup: (groupIndex: number, cwd: string, options?: { title?: string; command?: string; args?: string[] }): string => {
    tabCounter++
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const pg = makeTerminalPaneGroup(pgId, cwd, options)
    const title = options?.title
      ?? (options?.command ? options.command.split('/').pop()?.split(' ')[0] ?? `Terminal ${tabCounter}` : `Terminal ${tabCounter}`)

    const tab: Tab = {
      id: tabId,
      title,
      mosaicTree: pgId,
      paneGroups: new Map([[pgId, pg]])
    }

    if (groupIndex === 0) {
      // Group 0 = main tabs
      set((state) => ({ tabs: [...state.tabs, tab], activeTabId: tabId }))
    } else {
      set((state) => {
        const newGroups = [...state.extraGroups]
        const gi = groupIndex - 1
        if (gi >= 0 && gi < newGroups.length) {
          newGroups[gi] = {
            tabs: [...newGroups[gi].tabs, tab],
            activeTabId: tabId
          }
        }
        return { extraGroups: newGroups }
      })
    }

    // 0.38.0 Commit 4 — no cross-window broadcast. The daemon's
    // `/cli/sessions/events` WS push delivers `session_added` to other
    // windows once the v2 spawn registers the PTY.

    return pgId
  },

  removeTabFromGroup: (groupIndex: number, tabId: string) => {
    if (groupIndex === 0) {
      get().removeTab(tabId)
      return
    }

    const state = get()
    const gi = groupIndex - 1
    if (gi < 0 || gi >= state.extraGroups.length) return

    const group = state.extraGroups[gi]
    // Kill PTYs
    const tab = group.tabs.find((t) => t.id === tabId)
    if (tab) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            closeTerminalForRenderer(item.data as TerminalItemData)
          }
        }
      }
    }

    const newTabs = group.tabs.filter((t) => t.id !== tabId)
    let newActiveId = group.activeTabId
    if (group.activeTabId === tabId) {
      const idx = group.tabs.findIndex((t) => t.id === tabId)
      newActiveId = newTabs.length > 0 ? newTabs[Math.min(idx, newTabs.length - 1)].id : null
    }

    const newGroups = [...state.extraGroups]
    newGroups[gi] = { tabs: newTabs, activeTabId: newActiveId }
    set({ extraGroups: newGroups })
  },

  setActiveTabInGroup: (groupIndex: number, tabId: string) => {
    // Track in nav history
    const state = get()
    const currentActive = groupIndex === 0
      ? state.activeTabId
      : state.extraGroups[groupIndex - 1]?.activeTabId
    if (currentActive && currentActive !== tabId) {
      const history = state.navHistory.slice(0, state.navIndex + 1)
      history.push(currentActive)
      set({ navHistory: history, navIndex: history.length })
    }

    if (groupIndex === 0) {
      set({ activeTabId: tabId })
      // per-client-view-state.md (Phase 2) — persist the user's PRIMARY
      // (group-0) selection into the per-client store so a cold load /
      // remote adoption restores THIS client's last selection, not a peer's.
      // Keyed by paneGroup signature (the stable identity that survives a
      // restore's tab-id re-mint). Group-0 paneGroupIds are preserved on
      // restore ("Reuse the saved ID"), so the signature round-trips.
      persistGroup0Selection(state, tabId)
      return
    }
    set((state) => {
      const newGroups = [...state.extraGroups]
      const gi = groupIndex - 1
      if (gi >= 0 && gi < newGroups.length) {
        newGroups[gi] = { ...newGroups[gi], activeTabId: tabId }
      }
      return { extraGroups: newGroups }
    })
  },

  moveTabToGroup: (fromGroup: number, toGroup: number, tabId: string) => {
    if (fromGroup === toGroup) return
    const state = get()

    // Get the tab from the source group
    let tab: Tab | undefined
    if (fromGroup === 0) {
      tab = state.tabs.find((t) => t.id === tabId)
    } else {
      const gi = fromGroup - 1
      tab = state.extraGroups[gi]?.tabs.find((t) => t.id === tabId)
    }
    if (!tab) return

    // Remove from source (without killing PTYs — we're moving, not closing)
    if (fromGroup === 0) {
      const newTabs = state.tabs.filter((t) => t.id !== tabId)
      let newActiveId = state.activeTabId
      if (state.activeTabId === tabId) {
        const idx = state.tabs.findIndex((t) => t.id === tabId)
        newActiveId = newTabs.length > 0 ? newTabs[Math.min(idx, newTabs.length - 1)].id : null
      }
      set({ tabs: newTabs, activeTabId: newActiveId })
    } else {
      const gi = fromGroup - 1
      const group = state.extraGroups[gi]
      if (!group) return
      const newTabs = group.tabs.filter((t) => t.id !== tabId)
      let newActiveId = group.activeTabId
      if (group.activeTabId === tabId) {
        const idx = group.tabs.findIndex((t) => t.id === tabId)
        newActiveId = newTabs.length > 0 ? newTabs[Math.min(idx, newTabs.length - 1)].id : null
      }
      const newGroups = [...state.extraGroups]
      newGroups[gi] = { tabs: newTabs, activeTabId: newActiveId }
      set({ extraGroups: newGroups })
    }

    // Add to target group
    const updatedState = get()
    if (toGroup === 0) {
      set({ tabs: [...updatedState.tabs, tab], activeTabId: tab.id })
    } else {
      const gi = toGroup - 1
      const newGroups = [...updatedState.extraGroups]
      if (gi >= 0 && gi < newGroups.length) {
        newGroups[gi] = {
          tabs: [...newGroups[gi].tabs, tab],
          activeTabId: tab.id
        }
        set({ extraGroups: newGroups })
      }
    }
  },

  getGroupTabs: (groupIndex: number): { tabs: Tab[], activeTabId: string | null } => {
    const state = get()
    if (groupIndex === 0) return { tabs: state.tabs, activeTabId: state.activeTabId }
    const gi = groupIndex - 1
    if (gi >= 0 && gi < state.extraGroups.length) return state.extraGroups[gi]
    return { tabs: [], activeTabId: null }
  },

  // ── Layout persistence per workspace ──────────────────────────────────

  activeWorkspaceKey: null,
  activeProjectId: null,
  activeWorkspaceId: null,
  backgroundWorkspaces: {},
  workspaceLayouts: {},

  serializeCurrentLayout: (): SerializedLayout => {
    const state = get()
    const serializedTabs = state.tabs.map(serializeTab)
    // per-client-view-state.md (Phase 1) — the canonical layout carries only
    // STRUCTURE. `activeTabId` (top-level AND per-split-group) is per-client
    // VIEW state and is intentionally OMITTED so a save can't drag this
    // client's selection onto peers that adopt the layout.
    const serializedExtraGroups = state.extraGroups.map((group) => ({
      tabs: group.tabs.map(serializeTab),
    }))
    return {
      version: LAYOUT_SCHEMA_VERSION,
      tabs: serializedTabs,
      extraGroups: serializedExtraGroups.length > 0 ? serializedExtraGroups : undefined,
      splitCount: state.splitCount > 1 ? state.splitCount : undefined,
      activeGroupIndex: state.activeGroupIndex > 0 ? state.activeGroupIndex : undefined,
    }
  },

  restoreLayout: (layout: SerializedLayout, cwd: string) => {
    try {
    // 0.38.0 — v1→v2 migration. Walks all paneGroup items and drops
    // daemon-owned fields from terminal items. Daemon-owned data is
    // refilled by `reconcileWithDaemon` after restoration. The on-disk
    // copy converges to v2 on the next `saveLayoutForWorkspace` call,
    // which `loadLayoutForWorkspace` triggers when migration happens.
    migrateLayoutToV2(layout)

    const restoredTabs: Tab[] = layout.tabs.map((serializedTab) => {
      const paneGroups = new Map<string, PaneGroup>()

      // We need to remap paneGroup IDs: old serialized IDs -> new UUIDs
      // because terminal items need fresh terminalIds (old PTYs are dead).
      const idMap = new Map<string, string>()

      // Handle both new format (paneGroups) and legacy format (panes)
      const serializedPaneGroups = serializedTab.paneGroups
        ?? convertLegacyPanes((serializedTab as any).panes)

      if (!serializedPaneGroups || typeof serializedPaneGroups !== 'object') {
        console.warn('[tabs] Corrupted layout: missing paneGroups, creating fresh tab')
        const pgId = crypto.randomUUID()
        const pg = makeTerminalPaneGroup(pgId, cwd)
        paneGroups.set(pgId, pg)
        idMap.set('default', pgId)
      } else {
      for (const [oldPgId, serializedPg] of Object.entries(serializedPaneGroups)) {
        // Reuse the saved ID — if a background PTY was already spawned with it
        // (e.g. delegate), the backend skips re-creation and connects to the
        // existing process. If no PTY exists, a new one is created with this ID.
        const newPgId = oldPgId
        idMap.set(oldPgId, newPgId)

        const rawItems = Array.isArray(serializedPg?.items) ? serializedPg.items : []
        const items: Item[] = rawItems.map((si) => {
          if (si.type === 'terminal') {
            // After migrateLayoutToV2, terminal items are v2 shape:
            // only paneGroupId + heartbeat metadata. cwd/command/args/
            // sessionId/renderer come from the daemon at reconcile
            // time. Default to undefined so TerminalPane attaches to
            // the existing daemon PTY by paneGroupId; if no PTY
            // exists, it spawns a fresh shell at the workspace cwd.
            const t = si as SerializedTerminalItemV2
            return {
              id: crypto.randomUUID(),
              type: 'terminal' as const,
              data: {
                terminalId: newPgId,
                cwd,
                renderer: currentRenderer(),
                // Heartbeat metadata stays in the v2 schema — daemon's
                // list endpoint doesn't expose these, and
                // closeTerminalForRenderer depends on them to keep
                // the PTY alive on tab close (close-as-minimize).
                heartbeatName: t.heartbeatName,
                projectPath: t.projectPath,
                surfacedAgentName: t.surfacedAgentName,
                attachAgentName: t.attachAgentName,
              },
            }
          } else if (si.type === 'agent') {
            const restoredSection = si.section ?? 'inbox'
            return {
              id: crypto.randomUUID(),
              type: 'agent' as const,
              data: {
                agentName: si.agentName ?? '',
                projectPath: si.projectPath ?? cwd,
                section: restoredSection,
                // 0.37.12 — restore the pinned chat tab's Claude
                // session id (when present). AgentChatPane reads
                // this as a hint and resumes the same session
                // immediately without round-tripping
                // k2so_agents_resume_chat_args. Absent on rows
                // serialized before 0.37.12 — AgentChatPane falls
                // back to the daemon lookup in that case.
                //
                // 0.37.12 P1C scrub: only restore sessionId for
                // the chat tab. Inbox tabs (and any other
                // non-chat agent surface) don't render Claude, so
                // a stamped sessionId there is unused garbage.
                // Pre-fix layouts may carry a leaked value from
                // the broader stampAgentSessionId match. Drop it
                // on restore so the next serialize doesn't
                // round-trip the stale data.
                sessionId: restoredSection === 'chat' ? si.sessionId : undefined,
              },
            }
          } else {
            return {
              id: crypto.randomUUID(),
              type: 'file-viewer' as const,
              data: {
                filePath: si.filePath ?? '',
                scrollTop: si.scrollTop,
                cursorPos: si.cursorPos,
              },
              pinned: si.pinned ?? false,
            }
          }
        })

        // Ensure at least one item per pane group
        if (items.length === 0) {
          items.push({
            id: crypto.randomUUID(),
            type: 'terminal',
            data: { terminalId: newPgId, cwd, renderer: currentRenderer() },
          })
        }

        const clampedIndex = Math.max(0, Math.min(serializedPg?.activeItemIndex ?? 0, items.length - 1))
        paneGroups.set(newPgId, {
          id: newPgId,
          items,
          activeItemIndex: clampedIndex,
        })
      }
      }

      // Remap the mosaic tree IDs
      const remappedTree = remapMosaicIds(serializedTab.mosaicTree, idMap)

      tabCounter++
      return {
        id: crypto.randomUUID(),
        title: serializedTab.title,
        mosaicTree: remappedTree,
        paneGroups,
        ...(serializedTab.isSystemAgent ? { isSystemAgent: true } : {}),
        ...(serializedTab.isPinnedFile ? { isPinnedFile: true } : {}),
      }
    })

    // per-client-view-state.md (Phase 1+2) — selection is NO LONGER read from
    // the shared layout's leaked `activeTabId` (that hijacked peers). It comes
    // from this client's per-client selected-tabs store, matched by paneGroup
    // SIGNATURE (the stable identity that survives restore's tab-id re-mint).
    // Fallbacks: the saved selection's tab no longer exists → first tab; a
    // brand-new client with no saved selection → first tab (preserves #658
    // cold-boot pinned-chat, which is the first/system tab).
    const restoredActiveTabId = resolveRestoredSelection(get(), restoredTabs)

    // Restore extra groups (split columns)
    const restoredExtraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> = []
    if (layout.extraGroups) {
      for (const group of layout.extraGroups) {
        const groupTabs = group.tabs.map((serializedTab) => {
          // Use the same restore logic as group 0
          const paneGroups = new Map<string, PaneGroup>()
          const idMap = new Map<string, string>()
          const serializedPaneGroups = serializedTab.paneGroups
            ?? convertLegacyPanes((serializedTab as any).panes)
          if (serializedPaneGroups && typeof serializedPaneGroups === 'object') {
            for (const [oldPgId, serializedPg] of Object.entries(serializedPaneGroups)) {
              const newPgId = crypto.randomUUID()
              idMap.set(oldPgId, newPgId)
              const rawItems = Array.isArray(serializedPg?.items) ? serializedPg.items : []
              const items: Item[] = rawItems.map((si) => {
                if (si.type === 'terminal') {
                  // v2: daemon owns command/args/cwd/sessionId/renderer.
                  // Reconcile fills them in after restore.
                  const t = si as SerializedTerminalItemV2
                  return {
                    id: crypto.randomUUID(),
                    type: 'terminal' as const,
                    data: {
                      terminalId: newPgId,
                      cwd,
                      renderer: currentRenderer(),
                      heartbeatName: t.heartbeatName,
                      projectPath: t.projectPath,
                      surfacedAgentName: t.surfacedAgentName,
                      attachAgentName: t.attachAgentName,
                    },
                  }
                } else if (si.type === 'agent') {
                  return {
                    id: crypto.randomUUID(),
                    type: 'agent' as const,
                    data: { agentName: si.agentName ?? '', projectPath: si.projectPath ?? cwd },
                  }
                } else {
                  return {
                    id: crypto.randomUUID(),
                    type: 'file-viewer' as const,
                    data: {
                      filePath: si.filePath ?? '',
                      scrollTop: si.scrollTop,
                      cursorPos: si.cursorPos,
                    },
                    pinned: si.pinned ?? false,
                  }
                }
              })
              if (items.length === 0) {
                items.push({ id: crypto.randomUUID(), type: 'terminal', data: { terminalId: newPgId, cwd, renderer: currentRenderer() } })
              }
              const clampedIndex = Math.max(0, Math.min(serializedPg?.activeItemIndex ?? 0, items.length - 1))
              paneGroups.set(newPgId, { id: newPgId, items, activeItemIndex: clampedIndex })
            }
          }
          tabCounter++
          return {
            id: crypto.randomUUID(),
            title: serializedTab.title,
            mosaicTree: remapMosaicIds(serializedTab.mosaicTree, idMap),
            paneGroups,
          }
        })
        // per-client-view-state.md (Phase 1) — split-group selection is also
        // per-client VIEW state and is no longer read from the (now omitted)
        // serialized `group.activeTabId`. Default to the group's first tab;
        // the user's split focus re-establishes locally as they interact. (The
        // per-client store tracks only the primary group-0 selection.)
        restoredExtraGroups.push({
          tabs: groupTabs,
          activeTabId: groupTabs.length > 0 ? groupTabs[0].id : null,
        })
      }
    }

    set({
      tabs: restoredTabs,
      activeTabId: restoredActiveTabId,
      extraGroups: restoredExtraGroups,
      splitCount: layout.splitCount ?? 1,
      activeGroupIndex: layout.activeGroupIndex ?? 0,
    })
    } catch (err) {
      console.error('[tabs] Failed to restore layout, falling back to fresh tab:', err)
      tabCounter++
      const tabId = crypto.randomUUID()
      const paneGroupId = crypto.randomUUID()
      const pg = makeTerminalPaneGroup(paneGroupId, cwd)
      const tab: Tab = {
        id: tabId,
        title: `Terminal ${tabCounter}`,
        mosaicTree: paneGroupId,
        paneGroups: new Map([[paneGroupId, pg]]),
      }
      set({ tabs: [tab], activeTabId: tabId, extraGroups: [], splitCount: 1, activeGroupIndex: 0 })
    }
  },

  saveLayoutForWorkspace: (projectId: string, workspaceId: string) => {
    // 0.39.39 (#676/#677) — silent remote-reorder adoption: never echo a save
    // back while applying a peer's layout (would re-broadcast and ping-pong).
    if (isLayoutSaveSuppressed()) return
    const state = get()
    const key = `${projectId}:${workspaceId}`

    if (state.tabs.length === 0) {
      // Save empty state so next open shows the empty workspace hints
      // instead of restoring a stale session
      const { [key]: _, ...remaining } = state.workspaceLayouts
      set({ workspaceLayouts: remaining })
      // POST body is camelCase (LayoutDeleteBody `#[serde(rename_all =
      // "camelCase")]`); no cross-window sync — layouts aren't a synced
      // surface (the old Tauri layout commands emitted nothing).
      daemonCliPost('workspace-layouts/delete', { projectId, workspaceId }).catch(() => {})
      return
    }

    const layout = state.serializeCurrentLayout()
    set({ workspaceLayouts: { ...state.workspaceLayouts, [key]: layout } })

    // Persist to SQLite via the host-aware daemon route. Capture the
    // monotonic `revision` (#677.3) so a later remote reorder's broadcast
    // can be compared against our base and a stale local write skipped.
    daemonCliPost<{ success?: boolean; revision?: number }>('workspace-layouts/save', {
      projectId,
      workspaceId,
      layoutJson: JSON.stringify(layout),
    })
      .then((res) => recordLayoutRevision(key, res?.revision))
      .catch((err) => {
        console.error('[tabs] Failed to persist workspace layout:', err)
      })
  },

  loadLayoutForWorkspace: async (projectId: string, workspaceId: string, cwd: string): Promise<void> => {
    const key = `${projectId}:${workspaceId}`
    // per-client-view-state.md (Phase 2) — thread the workspace ids into the
    // store BEFORE `restoreLayout` runs (it reads them via
    // `resolveRestoredSelection` to source this client's selection) and so
    // later group-0 selection writes (`setActiveTabInGroup`) have the key.
    set({ activeProjectId: projectId, activeWorkspaceId: workspaceId })
    const savedLayout = get().workspaceLayouts[key]
    // Kill any existing PTYs in the active view before restoring
    get().clearAllTabs()

    // 0.38.0 Commit 4 — tear down the previous workspace's WS push
    // subscription before switching. The new one gets opened after
    // the initial reconcile so any race between "subscribe before
    // load" and "adopt during load" is impossible.
    tearDownActiveWorkspaceSubscription()

    // 0.38.0 — daemon-authoritative reconciliation. Query the daemon
    // for live PTYs in this workspace and adopt any `tab-<X>` session
    // that isn't already surfaced in a restored tab. This is what
    // makes opening a workspace in a second window (focus, "New
    // Window") show the same N tabs as the daemon's session map,
    // rather than the renderer's stale layout snapshot.
    //
    // Adoption only in this commit. Drop-dead-tabs would also be
    // correct under the invariant but risks surprising data loss if
    // the daemon is briefly slow/unreachable at mount; layering it
    // in is a follow-up.
    const reconcileWithDaemon = async (): Promise<void> => {
      if (get().activeWorkspaceKey !== key) return
      const sessions = await fetchDaemonSessions(cwd)
      if (sessions === null) {
        // Daemon unreachable — leave layout alone, but still attempt
        // to open the push subscription. `subscribeToWorkspaceSessionEvents`
        // has its own retry-with-backoff, so once the daemon comes
        // back the renderer learns about live sessions via push
        // (no second reconcile pass needed for the common case).
        subscribeForActiveWorkspace(key, projectId, workspaceId, cwd)
        return
      }
      if (get().activeWorkspaceKey !== key) return // user switched mid-query

      // Index daemon-side `tab-<X>` sessions by paneGroupId for fast lookup
      // when refreshing existing terminal items and detecting orphans.
      const daemonByPgId = new Map<string, DaemonSessionRow>()
      for (const s of sessions) {
        if (s.isV2 && typeof s.agentName === 'string' && s.agentName.startsWith('tab-')) {
          daemonByPgId.set(s.agentName.slice(4), s)
        }
      }

      const state = get()
      const surfacedPgIds = new Set<string>()
      for (const tab of state.tabs) {
        tab.paneGroups.forEach((_, pgId) => surfacedPgIds.add(pgId))
      }

      // 0.38.0 v2 — refresh existing terminal items with daemon-owned
      // command/args/cwd. After v1→v2 migration, the renderer no longer
      // carries this data in layout JSON; the daemon's list endpoint is
      // the canonical source. Mutates TerminalItemData in place so live
      // TerminalPane components pick up the new values via the
      // subscription that fires from the `set` call below.
      let refreshedItems = 0
      for (const tab of state.tabs) {
        for (const [pgId, pg] of tab.paneGroups) {
          const session = daemonByPgId.get(pgId)
          if (!session) continue
          for (const item of pg.items) {
            if (item.type !== 'terminal') continue
            const d = item.data as TerminalItemData
            const nextCwd = session.cwd || d.cwd
            const nextCmd = session.command ?? d.command
            const nextArgs = session.args.length > 0 ? session.args : d.args
            const nextSession = session.sessionId || d.sessionId
            if (
              d.cwd !== nextCwd ||
              d.command !== nextCmd ||
              !arraysEqual(d.args, nextArgs) ||
              d.sessionId !== nextSession
            ) {
              d.cwd = nextCwd
              d.command = nextCmd
              d.args = nextArgs
              d.sessionId = nextSession
              refreshedItems += 1
            }
          }
        }
      }

      const adopted: Tab[] = []
      for (const [pgId, session] of daemonByPgId) {
        if (surfacedPgIds.has(pgId)) continue
        adopted.push(
          buildAdoptedTerminalTab({
            paneGroupId: pgId,
            cwd: session.cwd || cwd,
            command: session.command ?? undefined,
            args: session.args.length > 0 ? session.args : undefined,
            sessionId: session.sessionId,
          }),
        )
      }

      const changed = adopted.length > 0 || refreshedItems > 0
      if (changed) {
        if (adopted.length > 0) {
          console.warn(`[tabs] reconcile: adopted ${adopted.length} orphan daemon PTY(s) for ${key}`)
        }
        if (refreshedItems > 0) {
          console.warn(`[tabs] reconcile: refreshed ${refreshedItems} terminal item(s) with daemon data for ${key}`)
        }
        // Shallow-copy tabs so Zustand subscribers see the change even
        // when only TerminalItemData fields mutated in place.
        set({ tabs: [...state.tabs, ...adopted] })
        // Only persist when adopt changed the on-disk tab set. Refresh-only
        // updates fill in transient fields (cwd/command/args/sessionId)
        // that v2 serialization intentionally drops, so the rewrite would
        // be byte-identical to disk — saving it every restore is a
        // wasted SQL write and adds log noise.
        if (adopted.length > 0) {
          get().saveLayoutForWorkspace(projectId, workspaceId)
        }
      }

      // 0.38.0 Commit 4 — now that the renderer's state is in sync
      // with the daemon's current snapshot, open the push subscription
      // so any subsequent session_added / session_removed surfaces
      // in this window without polling.
      subscribeForActiveWorkspace(key, projectId, workspaceId, cwd)
    }

    // 0.38.0 — heal duplicate tab rows that pre-0.38.0 builds may have
    // written into workspace_layouts.layout_json. Collapses tabs sharing
    // paneGroup-id sets (which all attach to the same daemon PTY) and
    // re-saves so the corrupt shape doesn't survive another open.
    const healAndSave = (): void => {
      const state = get()
      const group0 = dedupTabsBySignature(state.tabs)
      const cleanedExtraGroups = state.extraGroups.map((g) => {
        const r = dedupTabsBySignature(g.tabs)
        const newActive = g.activeTabId && r.idMap.has(g.activeTabId)
          ? r.idMap.get(g.activeTabId)!
          : g.activeTabId
        return { tabs: r.tabs, activeTabId: newActive, removed: r.removed }
      })
      const totalRemoved = group0.removed + cleanedExtraGroups.reduce((n, g) => n + g.removed, 0)
      if (totalRemoved > 0) {
        const newActive = state.activeTabId && group0.idMap.has(state.activeTabId)
          ? group0.idMap.get(state.activeTabId)!
          : state.activeTabId
        set({
          tabs: group0.tabs,
          activeTabId: newActive,
          extraGroups: cleanedExtraGroups.map((g) => ({ tabs: g.tabs, activeTabId: g.activeTabId })),
        })
        console.warn(`[tabs] healed ${totalRemoved} duplicate tab row(s) on restore for ${key}`)
        get().saveLayoutForWorkspace(projectId, workspaceId)
      }
    }

    if (savedLayout && savedLayout.tabs && savedLayout.tabs.length > 0) {
      const wasPreV2 = (savedLayout.version ?? 1) < LAYOUT_SCHEMA_VERSION
      get().restoreLayout(savedLayout, cwd)
      set({ activeWorkspaceKey: key })
      healAndSave()
      if (wasPreV2) {
        // v1 → v2 conversion happened inside restoreLayout. Persist the
        // migrated shape so the disk copy converges and the next open
        // is a pure-v2 path.
        console.warn(`[tabs] migrated workspace_layouts to v2 for ${key}`)
        get().saveLayoutForWorkspace(projectId, workspaceId)
      }
      void reconcileWithDaemon()
    } else {
      // Set key early so race-condition guards work
      set({ activeWorkspaceKey: key })

      // #658 — AWAIT the DB load so the returned promise resolves only
      // AFTER `restoreLayout` (or `launchDefaultAgent`) has populated
      // tabs + activeTabId + the restored chat sessionId. Cold-boot
      // callers await `loadLayoutForWorkspace` before running the
      // pinned-tab ensure, so the restored Chat tab (active, with its
      // saved sessionId) wins the ordering race. The daemon reconcile
      // pass below stays fire-and-forget — it's a background refresh,
      // not part of the initial restore the caller waits on.
      try {
        // Try loading from DB. GET query params are snake_case (the daemon
        // reads `project_id`/`workspace_id`); the route returns the layout
        // JSON string or null (`Option<String>` serialized).
        const json = await daemonCliGet<string | null>('workspace-layouts/load', { project_id: projectId, workspace_id: workspaceId })
        // Guard: bail if user already switched to a different workspace
        if (get().activeWorkspaceKey !== key) return

        if (json) {
          try {
            const layout = JSON.parse(json) as SerializedLayout
            if (layout.tabs && layout.tabs.length > 0) {
              const wasPreV2 = (layout.version ?? 1) < LAYOUT_SCHEMA_VERSION
              set({ workspaceLayouts: { ...get().workspaceLayouts, [key]: layout } })
              get().restoreLayout(layout, cwd)
              healAndSave()
              if (wasPreV2) {
                console.warn(`[tabs] migrated workspace_layouts to v2 for ${key}`)
                get().saveLayoutForWorkspace(projectId, workspaceId)
              }
              void reconcileWithDaemon()
              return
            }
          } catch (err) {
            console.error('[tabs] Failed to parse DB layout:', err)
          }
        }
        // No saved layout — auto-launch default agent after short delay
        // (launchDefaultAgent has its own adoption flow for the
        // empty-workspace case; reconcileWithDaemon's job is the
        // existing-layout-plus-orphan-PTY case)
        get().launchDefaultAgent(key, cwd)
        // 0.38.0 Commit 4 — even on the empty-workspace path we
        // need the WS push subscription so subsequent Cmd+T or
        // mobile-companion spawns surface here without polling.
        subscribeForActiveWorkspace(key, projectId, workspaceId, cwd)
      } catch {
        // DB unavailable — auto-launch default agent
        get().launchDefaultAgent(key, cwd)
        subscribeForActiveWorkspace(key, projectId, workspaceId, cwd)
      }
    }
  },

  loadWorkspaceSessionsFromDb: async () => {
    try {
      // The daemon's `workspace-layouts/load-all` route serializes
      // `db_ops::WorkspaceLayout` with `#[serde(rename_all = "camelCase")]`,
      // emitting exactly `{ projectId, workspaceId, layoutJson }`. The old
      // Tauri `workspace_layout_load_all` did an `Into::into` into a
      // field-identical wrapper struct (also camelCase) — a pure rename
      // with no shape change — so the raw daemon response already matches
      // this type and needs NO post-fetch transform.
      const sessions = await daemonCliGet<Array<{ projectId: string, workspaceId: string, layoutJson: string }>>('workspace-layouts/load-all')
      const layouts: Record<string, SerializedLayout> = {}
      for (const session of sessions) {
        try {
          layouts[`${session.projectId}:${session.workspaceId}`] = JSON.parse(session.layoutJson)
        } catch { /* skip corrupt entries */ }
      }
      set({ workspaceLayouts: layouts })
      // Phase 2.5 fix (finding #547): mark the baseline as loaded so
      // the reconnect-bus retry stops firing. Empty list is success.
      hasLoadedWorkspaceSessions = true
    } catch (err) {
      console.error('[tabs] Failed to load workspace sessions from DB:', err)
    }
  },

  /** @deprecated Use loadWorkspaceSessionsFromDb instead */
  loadWorkspaceLayoutsFromSettings: async () => {
    // Redirect to new DB-backed loader
    return get().loadWorkspaceSessionsFromDb()
  },

  clearAllTabs: () => {
    // 0.38.0 commit 5 — view-clear only. Previously this looped every
    // terminal item and called `closeTerminalForRenderer` (which routes
    // v2 sessions to `closeV2Session`, unregistering them from the
    // daemon's v2_session_map and killing the PTY).
    //
    // That made every workspace switch / focus-window mount destructive
    // to anyone else viewing the same workspace: the daemon's session
    // got killed, the renderer respawned a fresh one with the same
    // canonical agent_name, and any other window (main, another focus
    // window) was left holding a WS handle to a dead session_id —
    // "distinct forks of the same chat" from the user's perspective.
    //
    // Under the daemon-authoritative model, the renderer's tab list is
    // a *view*. Clearing the view must not affect the daemon's session
    // map. When TerminalPane components unmount, their grid WS closes
    // gracefully (subscriber detach); the daemon's
    // `Arc<DaemonPtySession>` stays alive in v2_session_map until
    // something explicit (user Close Tab, project deletion, daemon
    // restart) takes it down. Other viewers keep their subscriptions
    // and don't see a flicker.
    //
    // Explicit user closes still kill — those go through `removeTab` /
    // file-viewer-close / split-pane-replace, which call
    // `closeTerminalForRenderer` directly. Those call sites are
    // unchanged.
    set({ tabs: [], activeTabId: null, extraGroups: [], splitCount: 1, activeGroupIndex: 0 })
  },

  // Detect active CLI tool session IDs for all running terminals
  // across active tabs, extra groups, and background workspaces.
  detectAndSaveSessionIds: async () => {
    const state = get()
    let updated = false

    // Helper to detect sessions in a list of tabs
    const detectInTabs = async (tabs: Tab[]): Promise<void> => {
      for (const tab of tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type !== 'terminal') continue
            const d = item.data as TerminalItemData
            if (!d.command || d.sessionId) continue
            const toolConfig = RESUMABLE_CLI_TOOLS[d.command]
            if (!toolConfig) continue
            try {
              const r = await daemonCliGet<{ sessionId: string | null }>('chat/detect-active', {
                provider: toolConfig.provider,
                project_path: d.cwd,
              })
              const sessionId = r.sessionId
              if (sessionId) {
                ;(item.data as TerminalItemData).sessionId = sessionId
                updated = true
              }
            } catch (err) {
              console.error('[tabs] Failed to detect session for', d.command, err)
            }
          }
        }
      }
    }

    // Active group 0
    await detectInTabs(state.tabs)
    // Extra groups
    for (const group of state.extraGroups) {
      await detectInTabs(group.tabs)
    }
    // Background workspaces
    for (const snapshot of Object.values(state.backgroundWorkspaces)) {
      await detectInTabs(snapshot.tabs)
      for (const group of snapshot.extraGroups) {
        await detectInTabs(group.tabs)
      }
    }

    if (updated) {
      // Trigger re-render with shallow copies so serialization picks up mutations
      set({
        tabs: [...state.tabs],
        extraGroups: [...state.extraGroups],
        backgroundWorkspaces: { ...state.backgroundWorkspaces },
      })
    }
  },

  // ── Background workspace management ─────────────────────────────────

  launchDefaultAgent: (key: string, cwd: string) => {
    // 0.37.11 A9 Phase 4b — daemon-side session adoption.
    //
    // Before spawning a default, ask the daemon if it already has live
    // v2 sessions for this workspace. A second window opening the same
    // workspace (focus window, "new window", etc.) adopts the existing
    // PTYs instead of spawning duplicates.
    //
    // Only adopt `tab-<terminalId>` sessions — those are user-spawned
    // Cmd+T / launchDefaultAgent tabs. System-spawned sessions
    // (workspace agent panel, heartbeats, delegate) live under their
    // canonical agent names and are owned by other surfaces.
    void (async () => {
      if (get().activeWorkspaceKey !== key || get().tabs.length > 0) return

      try {
        const json = await invoke<string>('k2so_sessions_list_for_workspace', { path: cwd })
        const sessions: Array<{
          sessionId: string
          agentName: string
          command: string | null
          args: string[]
          cwd: string
          isV2: boolean
        }> = JSON.parse(json)

        const adoptable = sessions.filter(
          (s) => s.isV2 && typeof s.agentName === 'string' && s.agentName.startsWith('tab-'),
        )

        if (
          adoptable.length > 0 &&
          get().activeWorkspaceKey === key &&
          get().tabs.length === 0
        ) {
          const adoptedTabs: Tab[] = adoptable.map((s) => {
            const terminalId = s.agentName.slice(4)
            const paneGroupId = terminalId
            const tabId = crypto.randomUUID()
            const pg = makeTerminalPaneGroup(
              paneGroupId,
              s.cwd || cwd,
              s.command
                ? { command: s.command, args: s.args.length > 0 ? s.args : undefined }
                : undefined,
            )
            tabCounter++
            return {
              id: tabId,
              title: s.command ?? `Terminal ${tabCounter}`,
              mosaicTree: paneGroupId,
              paneGroups: new Map([[paneGroupId, pg]]),
            }
          })
          set({ tabs: adoptedTabs, activeTabId: adoptedTabs[0].id })
          return
        }
      } catch (err) {
        console.warn('[tabs] adoption query failed; falling through to default spawn:', err)
      }

      // ── No adoptable sessions — fresh default-agent spawn ──
      setTimeout(async () => {
        if (get().activeWorkspaceKey !== key || get().tabs.length !== 0) return
        tabCounter++
        const tabId = crypto.randomUUID()
        const paneGroupId = crypto.randomUUID()

        // Look up default agent preset
        let agentOpts: { command?: string; args?: string[]; title?: string } = {}
        try {
          const defaultAgent = useSettingsStore.getState().defaultAgent
          if (defaultAgent && _presetsStoreRef) {
            const presets = _presetsStoreRef().presets
            const preset = presets.find((p: any) => {
              const cmd = p.command.split(/\s+/)[0]
              return cmd === defaultAgent && p.enabled
            })
            if (preset) {
              const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
              const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
              agentOpts = { command: cleaned[0], args: cleaned.slice(1), title: preset.label }
            }
          }
        } catch { /* fall back to plain terminal */ }

        // Resume previous session if this is a resumable CLI tool (e.g. Claude)
        if (agentOpts.command) {
          const toolConfig = RESUMABLE_CLI_TOOLS[agentOpts.command]
          if (toolConfig) {
            try {
              const r = await daemonCliGet<{ sessionId: string | null }>('chat/detect-active', {
                provider: toolConfig.provider,
                project_path: cwd,
              })
              const sessionId = r.sessionId
              if (sessionId) {
                if (toolConfig.resumeSubcommand) {
                  agentOpts.args = [toolConfig.resumeSubcommand, sessionId]
                } else if (toolConfig.resumeFlag) {
                  const baseArgs = agentOpts.args ?? []
                  agentOpts.args = [...baseArgs, toolConfig.resumeFlag, sessionId]
                }
              }
            } catch { /* session detection failed — launch fresh */ }
          }
        }

        const pg = makeTerminalPaneGroup(paneGroupId, cwd, agentOpts.command ? { command: agentOpts.command, args: agentOpts.args } : undefined)
        const tab: Tab = {
          id: tabId,
          title: agentOpts.title || `Terminal ${tabCounter}`,
          mosaicTree: paneGroupId,
          paneGroups: new Map([[paneGroupId, pg]])
        }
        set({ tabs: [tab], activeTabId: tabId })
      }, 100)
    })()
  },

  stashWorkspace: (key: string) => {
    const state = get()
    // Issue #8 (0.39.13) — stashing the active workspace is the
    // authoritative "this workspace is no longer in the foreground"
    // point, so close its session-events WS here. This guarantees the
    // invariant "no active workspace ⇒ no session-events subscription":
    // a stash that ISN'T immediately followed by a restore (e.g.
    // `setActiveProject(null)`, which stashes then returns early) no
    // longer leaks the previous workspace's WS. The matching
    // `restoreWorkspace` re-opens a fresh subscription for the workspace
    // it brings to the foreground. Guarded on key match so an unrelated
    // stash can't tear down the active sub. `tearDownActiveWorkspaceSubscription`
    // is idempotent.
    if (activeSessionEventsKey === key) {
      tearDownActiveWorkspaceSubscription()
    }
    if (state.tabs.length === 0 && state.extraGroups.length === 0) {
      // Workspace is empty — clear background snapshot AND DB session so
      // restoreWorkspace doesn't resurrect old tabs from either source
      const { [key]: _, ...remaining } = state.backgroundWorkspaces
      const { [key]: _layout, ...remainingLayouts } = state.workspaceLayouts
      set({
        backgroundWorkspaces: remaining,
        workspaceLayouts: remainingLayouts,
        activeWorkspaceKey: null,
      })
      // Delete saved session from DB so loadLayoutForWorkspace falls through to launchDefaultAgent
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        daemonCliPost('workspace-layouts/delete', { projectId, workspaceId }).catch(() => {})
      }
      return
    }

    // Move active tabs into background (PTYs stay alive)
    set({
      backgroundWorkspaces: {
        ...state.backgroundWorkspaces,
        [key]: {
          tabs: state.tabs,
          extraGroups: state.extraGroups,
          splitCount: state.splitCount,
          activeGroupIndex: state.activeGroupIndex,
          activeTabId: state.activeTabId,
        }
      },
      // Clear active view (React unmounts, but PTYs stay alive in backend)
      tabs: [],
      activeTabId: null,
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeWorkspaceKey: null,
    })
  },

  restoreWorkspace: async (key: string, cwd: string): Promise<void> => {
    const state = get()
    const live = state.backgroundWorkspaces[key]
    if (live && (live.tabs.length > 0 || live.extraGroups.length > 0)) {
      // Live tabs with running PTYs — swap them in. The restored
      // `live.activeTabId` is THIS client's own stashed selection (per-client
      // by nature — the snapshot never left this window), so the fast path
      // keeps it as-is. We still thread the workspace ids
      // (per-client-view-state.md Phase 2) so later group-0 selection writes
      // key the per-client store.
      const { [key]: _, ...remaining } = state.backgroundWorkspaces
      const [liveProjectId, liveWorkspaceId] = key.split(':')
      set({
        tabs: live.tabs,
        activeTabId: live.activeTabId,
        extraGroups: live.extraGroups,
        splitCount: live.splitCount,
        activeGroupIndex: live.activeGroupIndex,
        backgroundWorkspaces: remaining,
        activeWorkspaceKey: key,
        activeProjectId: liveProjectId ?? null,
        activeWorkspaceId: liveWorkspaceId ?? null,
      })
      // Issue #8 (0.39.13) — session-events subscription handoff on the
      // live fast path. The slow path delegates to
      // `loadLayoutForWorkspace`, which tears down + re-subscribes; this
      // fast path used to do neither. The switch sequence is
      // `stashWorkspace(old)` → `restoreWorkspace(new)`, and
      // `stashWorkspace` does NOT touch the subscription — so taking
      // this branch left `activeSessionEventsUnsub` pointing at the
      // PREVIOUS workspace's WS (still open, leaking) while the
      // restored workspace had no subscription at all. Re-point it at
      // the restored workspace so exactly one workspace's session-events
      // WS is ever open. `subscribeForActiveWorkspace` tears down the
      // stale sub before opening the new one (idempotent), so this is
      // also a no-op-safe re-entry if we were already subscribed to
      // `key`.
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        subscribeForActiveWorkspace(key, projectId, workspaceId, cwd)
      } else {
        // Malformed key — at minimum don't leak the previous sub.
        tearDownActiveWorkspaceSubscription()
      }
      // Pinned agent tab is ensured by the projects store after restoreWorkspace
      return
    }

    // Safety: if stash didn't run (e.g. activeWorkspaceId was null), the old
    // tabs might still be in the active view. Clear them before restoring so
    // the new workspace doesn't inherit the previous workspace's tabs.
    if (state.tabs.length > 0 || state.extraGroups.length > 0) {
      console.warn('[tabs] restoreWorkspace: clearing %d lingering tabs', state.tabs.length)
      get().clearAllTabs()
    }

    // No live tabs — fall back to serialized layout (creates new PTYs).
    //
    // #681 (Bug A) — AWAIT the slow-path load so this promise resolves
    // only AFTER `loadLayoutForWorkspace` has cleared the old tabs, set
    // `activeWorkspaceKey`, and (for a brand-new workspace) kicked off
    // `launchDefaultAgent`. The new-workspace open paths in the projects
    // store (addProject / setActiveProject / setActiveWorkspace) await
    // restoreWorkspace before calling `ensurePinnedAgentTabForMode`,
    // mirroring the #658 cold-boot ordering: the pinned Chat + Inbox
    // tabs are created deterministically AFTER the workspace key is set,
    // so they appear on first open instead of only after a switch-away.
    const [projectId, workspaceId] = key.split(':')
    if (projectId && workspaceId) {
      await get().loadLayoutForWorkspace(projectId, workspaceId, cwd)
    }
    // Pinned agent tab is ensured by the projects store after restoreWorkspace
  },

  serializeAllWorkspaces: async (activeKey: string) => {
    const state = get()

    // Serialize + save current active workspace
    if (state.tabs.length > 0 || state.extraGroups.length > 0) {
      const layout = state.serializeCurrentLayout()
      const [projectId, workspaceId] = activeKey.split(':')
      if (projectId && workspaceId) {
        await daemonCliPost('workspace-layouts/save', {
          projectId,
          workspaceId,
          layoutJson: JSON.stringify(layout),
        }).catch((err) => console.error('[tabs] Failed to save active workspace:', err))
      }
    }

    // Serialize + save each background workspace
    for (const [key, snapshot] of Object.entries(state.backgroundWorkspaces)) {
      const layout = serializeSnapshot(snapshot)
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        await daemonCliPost('workspace-layouts/save', {
          projectId,
          workspaceId,
          layoutJson: JSON.stringify(layout),
        }).catch((err) => console.error('[tabs] Failed to save background workspace:', key, err))
      }
    }
  },

  clearBackgroundWorkspace: (key: string) => {
    const state = get()
    const snapshot = state.backgroundWorkspaces[key]
    if (!snapshot) return

    // Kill all PTYs in the background workspace
    for (const tab of snapshot.tabs) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            closeTerminalForRenderer(data)
          }
        }
      }
    }
    for (const group of snapshot.extraGroups) {
      for (const tab of group.tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const data = item.data as TerminalItemData
              closeTerminalForRenderer(data)
            }
          }
        }
      }
    }

    const { [key]: _, ...remaining } = state.backgroundWorkspaces
    set({ backgroundWorkspaces: remaining })
  },

  persistActiveWorkspace: () => {
    // 0.39.39 (#676/#677) — silent remote-reorder adoption. The autosave
    // subscription calls this synchronously inside `restoreLayout`'s `set(...)`
    // while adopting a peer's layout; bail before scheduling so no echoing
    // save fires. (Second guard inside the timer is defensive in case a future
    // caller schedules from within a suppressed window.)
    if (isLayoutSaveSuppressed()) return
    // Debounced save of the active workspace to DB
    if (persistDebounceTimer) clearTimeout(persistDebounceTimer)
    persistDebounceTimer = setTimeout(() => {
      if (isLayoutSaveSuppressed()) return
      const state = get()
      if (!state.activeWorkspaceKey) return
      if (state.tabs.length === 0 && state.extraGroups.length === 0) return

      const layout = state.serializeCurrentLayout()
      const key = state.activeWorkspaceKey
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        daemonCliPost<{ success?: boolean; revision?: number }>('workspace-layouts/save', {
          projectId,
          workspaceId,
          layoutJson: JSON.stringify(layout),
        })
          .then((res) => recordLayoutRevision(key, res?.revision))
          .catch((err) => console.error('[tabs] Auto-save failed:', err))
      }
    }, 1000)
  },

  addTabToWorkspace: (workspaceKey: string, cwd: string, options: { title: string; command: string; args: string[] }): string | null => {
    const state = get()

    // If this IS the active workspace, just add the tab directly
    if (state.activeWorkspaceKey === workspaceKey) {
      state.addTabToGroup(0, cwd, options)
      return null // terminal created by the live tab
    }

    // Otherwise, save a session to the DB for this workspace so the tab
    // is waiting when the user navigates there. Build a minimal v2
    // serialized layout — daemon owns the PTY's cwd/command/args so we
    // emit only metadata. `reconcileWithDaemon` fills them in when the
    // user lands on the workspace and the daemon's `tab-<pgId>` session
    // is discovered.
    const tabId = crypto.randomUUID()
    const pgId = crypto.randomUUID()
    const terminalId = pgId

    const layout: SerializedLayout = {
      version: LAYOUT_SCHEMA_VERSION,
      tabs: [{
        id: tabId,
        title: options.title,
        mosaicTree: pgId,
        paneGroups: {
          [pgId]: {
            id: pgId,
            items: [{
              id: crypto.randomUUID(),
              type: 'terminal' as const,
              paneGroupId: pgId,
            } as SerializedTerminalItemV2],
            activeItemIndex: 0,
          }
        }
      }],
      activeTabId: tabId,
      splitCount: 1,
      activeGroupIndex: 0,
    }

    const [projectId, workspaceId] = workspaceKey.split(':')
    if (projectId && workspaceId) {
      daemonCliPost('workspace-layouts/save', {
        projectId,
        workspaceId,
        layoutJson: JSON.stringify(layout),
      }).catch((err) => console.error('[tabs] Failed to save agent tab to workspace:', err))
    }

    return pgId // terminal ID for background PTY spawning
  },
}))

// ── Tree utilities ───────────────────────────────────────────────────────

function remapMosaicIds(
  tree: MosaicNode<string> | null,
  idMap: Map<string, string>
): MosaicNode<string> | null {
  if (tree === null) return null
  if (typeof tree === 'string') {
    return idMap.get(tree) ?? tree
  }
  return {
    ...tree,
    first: remapMosaicIds(tree.first, idMap) as MosaicNode<string>,
    second: remapMosaicIds(tree.second, idMap) as MosaicNode<string>
  }
}

function replaceInTree(
  tree: MosaicNode<string> | null,
  targetId: string,
  replacement: MosaicNode<string>
): MosaicNode<string> | null {
  if (tree === null) return null
  if (typeof tree === 'string') {
    return tree === targetId ? replacement : tree
  }
  return {
    ...tree,
    first: replaceInTree(tree.first, targetId, replacement) as MosaicNode<string>,
    second: replaceInTree(tree.second, targetId, replacement) as MosaicNode<string>
  }
}

// ── 0.38.0 daemon-authoritative reconciliation ───────────────────────────

interface DaemonSessionRow {
  sessionId: string
  agentName: string
  command: string | null
  args: string[]
  cwd: string
  isV2: boolean
}

/** Query the daemon for live PTYs whose cwd is under `projectPath`.
 *  Returns the full session list including all kinds (Cmd+T tabs,
 *  pinned chat, heartbeats). Callers filter by `agentName` shape to
 *  decide which sessions map to which tab class.
 *
 *  Returns `null` (not `[]`) on query failure so callers can
 *  distinguish "daemon unreachable" from "workspace has no sessions"
 *  and skip reconciliation rather than treating an empty result as
 *  authoritative. */
async function fetchDaemonSessions(projectPath: string): Promise<DaemonSessionRow[] | null> {
  try {
    const json = await invoke<string>('k2so_sessions_list_for_workspace', { path: projectPath })
    return JSON.parse(json) as DaemonSessionRow[]
  } catch (err) {
    console.warn('[tabs] daemon list_sessions failed for', projectPath, err)
    return null
  }
}

/** Shallow array equality, treating `undefined` and `[]` as equivalent. */
function arraysEqual(a: string[] | undefined, b: string[] | undefined): boolean {
  const aLen = a?.length ?? 0
  const bLen = b?.length ?? 0
  if (aLen !== bLen) return false
  if (aLen === 0) return true
  // eslint-disable-next-line @typescript-eslint/no-non-null-assertion
  return a!.every((v, i) => v === b![i])
}

// ── 0.38.0 layout schema v2 migration ────────────────────────────────────

/** v1 terminal items embedded daemon-owned data (cwd / command / args /
 *  sessionId / renderer). v2 makes the daemon the single source of truth
 *  — terminal items only carry their paneGroupId (the canonical key) plus
 *  the heartbeat metadata the daemon's list endpoint doesn't yet expose.
 *
 *  Mutates `layout` in place. Returns whether anything changed (callers
 *  use this to trigger a re-save so the disk copy converges to v2).
 *  Idempotent — running on a v2 layout is a no-op. */
function migrateLayoutToV2(layout: SerializedLayout): boolean {
  if ((layout.version ?? 1) >= LAYOUT_SCHEMA_VERSION) return false

  const migrateTab = (tab: SerializedTab): void => {
    if (!tab.paneGroups || typeof tab.paneGroups !== 'object') return
    for (const [pgId, pg] of Object.entries(tab.paneGroups)) {
      if (!pg || !Array.isArray(pg.items)) continue
      pg.items = pg.items.map((si) => {
        if (!si || si.type !== 'terminal') return si
        // Drop v1 daemon-owned fields; daemon refills via reconcile.
        const v2: SerializedTerminalItemV2 = {
          id: si.id,
          type: 'terminal',
          paneGroupId: pgId,
          heartbeatName: (si as SerializedTerminalItemV1 | SerializedTerminalItemV2).heartbeatName,
          surfacedAgentName: (si as SerializedTerminalItemV1 | SerializedTerminalItemV2).surfacedAgentName,
          attachAgentName: (si as SerializedTerminalItemV1 | SerializedTerminalItemV2).attachAgentName,
          projectPath: (si as SerializedTerminalItemV1 | SerializedTerminalItemV2).projectPath,
        }
        return v2
      })
    }
  }

  for (const tab of layout.tabs ?? []) migrateTab(tab)
  if (layout.extraGroups) {
    for (const group of layout.extraGroups) {
      for (const tab of group.tabs ?? []) migrateTab(tab)
    }
  }
  layout.version = LAYOUT_SCHEMA_VERSION
  return true
}

// ── 0.38.0 layout heal-on-read ───────────────────────────────────────────

/** Canonical identity of a tab. Two tabs that point to the same set of
 *  paneGroup IDs are pointing at the same daemon-side session(s) and
 *  are therefore duplicates regardless of their renderer-side tab UUIDs.
 *  Pre-0.38.0, the sync:tabs-request broadcast race could leak duplicate
 *  rows into workspace_layouts; this signature is what we collapse on. */
function tabSignature(tab: Tab): string {
  return [...tab.paneGroups.keys()].sort().join(',')
}

/** Collapse tabs that share canonical identity. Returns the cleaned list
 *  plus an idMap so callers can remap any pointer (e.g. activeTabId)
 *  that referenced a removed duplicate to its surviving twin. */
function dedupTabsBySignature(
  tabs: Tab[]
): { tabs: Tab[]; idMap: Map<string, string>; removed: number } {
  const seen = new Map<string, Tab>()
  const idMap = new Map<string, string>()
  const kept: Tab[] = []
  for (const tab of tabs) {
    const sig = tabSignature(tab)
    const existing = seen.get(sig)
    if (existing) {
      idMap.set(tab.id, existing.id)
    } else {
      seen.set(sig, tab)
      kept.push(tab)
    }
  }
  return { tabs: kept, idMap, removed: tabs.length - kept.length }
}

// ── 0.38.0 Commit 4: daemon-push session adoption helpers ───────────────

/** Construct a new terminal Tab around a daemon-owned PTY. Shared
 *  by reconcileWithDaemon (initial restore catch-up) and the
 *  `session_added` push handler so both paths produce identical
 *  shapes. The caller is responsible for inserting the returned
 *  tab into state. */
function buildAdoptedTerminalTab(args: {
  paneGroupId: string
  cwd: string
  command?: string
  args?: string[]
  sessionId?: string
}): Tab {
  const pg = makeTerminalPaneGroup(
    args.paneGroupId,
    args.cwd,
    args.command !== undefined
      ? { command: args.command, args: args.args }
      : undefined,
  )
  // Stamp the daemon-owned sessionId onto the new TerminalItemData so
  // close-as-minimize cross-references work without a refresh round-trip.
  if (args.sessionId && pg.items[0]?.type === 'terminal') {
    (pg.items[0].data as TerminalItemData).sessionId = args.sessionId
  }
  tabCounter++
  return {
    id: crypto.randomUUID(),
    title: args.command ?? `Terminal ${tabCounter}`,
    mosaicTree: args.paneGroupId,
    paneGroups: new Map([[args.paneGroupId, pg]]),
  }
}

/** True when removing the tab is safe under the daemon-authoritative
 *  invariant. Skip pinned/system tabs and any tab whose paneGroups
 *  hold non-terminal items (agent panes, file viewers, etc.) — those
 *  surfaces have their own lifecycle and are not driven by the
 *  daemon's v2 session map. */
function tabIsDropCandidateForSessionRemoval(tab: Tab, removedPgId: string): boolean {
  if (tab.isSystemAgent) return false
  if (tab.paneGroups.size !== 1) return false
  if (!tab.paneGroups.has(removedPgId)) return false
  const pg = tab.paneGroups.get(removedPgId)
  if (!pg) return false
  // Must be terminal-only — agent panes / file viewers / etc. block.
  for (const item of pg.items) {
    if (item.type !== 'terminal') return false
  }
  return true
}

/** Look up whether a paneGroupId is already surfaced by any tab
 *  across both group 0 and extraGroups. Used by the `session_added`
 *  handler to dedupe — local Cmd+T fires `addTab` which already
 *  creates the tab; the daemon's subsequent `session_added` push
 *  would otherwise add a duplicate.
 *
 *  Returns true if any tab in any group already includes the paneGroupId. */
function isPaneGroupSurfaced(
  state: { tabs: Tab[]; extraGroups: Array<{ tabs: Tab[]; activeTabId: string | null }> },
  pgId: string,
): boolean {
  for (const tab of state.tabs) {
    if (tab.paneGroups.has(pgId)) return true
  }
  for (const group of state.extraGroups) {
    for (const tab of group.tabs) {
      if (tab.paneGroups.has(pgId)) return true
    }
  }
  return false
}

/** Open the daemon push subscription for the currently-loading
 *  workspace. Tears down any existing subscription first so callers
 *  don't need to worry about stacking. The handlers translate the
 *  daemon's lifecycle events into store mutations (tab adoption,
 *  tab drop). */
function subscribeForActiveWorkspace(
  key: string,
  projectId: string,
  workspaceId: string,
  cwd: string,
): void {
  // #672 — THE load-bearing invariant (PRD §4.3.1): surfacing a
  // workspace's chat to the client is an ACTIVATION. This function is the
  // single chokepoint every chat-surfacing path funnels through —
  // `loadLayoutForWorkspace` (initial open + host-switch restore +
  // daemon-unreachable fallback) and `restoreWorkspace` (workspace switch
  // / tab focus, incl. K2 Connect remote-open). Activating here (deduped,
  // capability-gated inside `activateProject`) guarantees "a client is
  // watching it ⇒ it is canonically Active ⇒ the daemon reaper won't touch
  // it" — which the OLD renderer reaper failed to do for remote-opens
  // (that was GH#22).
  activateProjectViaRef(projectId)

  tearDownActiveWorkspaceSubscription()
  activeSessionEventsKey = key

  // 0.39.39 (#676/#677) — open the workspace-scoped tab-title / tab-order
  // broadcast subscription alongside the session-events one (capability-
  // gated; against an older/remote daemon these events never arrive and the
  // renderer keeps its local-layout behavior). On (re)connect re-snapshot
  // the canonical tab titles so a rename missed during a drop is backfilled.
  if (serverSupports('daemon-broadcasts')) {
    void applyTabTitlesSnapshot(projectId)
    activeTabEventsUnsub = subscribeToWorkspaceTabEvents(cwd, {
      onTabTitleChanged: (event: TabTitleChangedEvent) => {
        // Match on project_id (the event's `project` is the project PATH
        // echoed, but the title store is keyed by tab id which is globally
        // unique within a workspace — apply to the surfaced tab directly).
        if (useTabsStore.getState().activeWorkspaceKey !== key) return
        useTabsStore.getState().applyDaemonTabTitle(event.tabId, event.title)
      },
      onTabOrderChanged: (event: TabOrderChangedEvent) => {
        if (useTabsStore.getState().activeWorkspaceKey !== key) return
        // Only the active workspace's row matters here.
        if (event.project !== projectId || event.workspace !== workspaceId) return
        const base = layoutRevisions.get(key) ?? 0
        if (event.revision <= base) {
          // Our own write (or an older one) — already reflected locally.
          // Still advance the base so a duplicate broadcast is a no-op.
          recordLayoutRevision(key, event.revision)
          return
        }
        // A remote client reordered ahead of our base — re-fetch the
        // canonical layout and adopt it (no silent last-write-wins clobber).
        recordLayoutRevision(key, event.revision)
        void refetchLayoutForRemoteReorder(key, projectId, workspaceId, cwd)
      },
      onHello: () => {
        if (useTabsStore.getState().activeWorkspaceKey !== key) return
        void applyTabTitlesSnapshot(projectId)
      },
    })
  }

  activeSessionEventsUnsub = subscribeToWorkspaceSessionEvents(cwd, {
    onAdded: (event: SessionAddedEvent) => {
      // Only adopt `tab-<paneGroupId>` sessions — pinned chat and
      // heartbeats live under their own canonical agent_names with
      // their own dedicated surfaces. Forwarded events still drive
      // the mobile companion, the renderer just ignores them.
      const pgId = event.pane_group_id
      if (!pgId || !event.agent_name.startsWith('tab-')) return
      // Workspace switched while the event was in flight — bail.
      if (useTabsStore.getState().activeWorkspaceKey !== key) return
      const state = useTabsStore.getState()
      if (isPaneGroupSurfaced(state, pgId)) return
      const tab = buildAdoptedTerminalTab({
        paneGroupId: pgId,
        cwd: event.workspace_path || cwd,
        command: event.command ?? undefined,
        args: event.args.length > 0 ? event.args : undefined,
        sessionId: event.session_id,
      })
      console.warn(`[tabs] session_added push — adopting paneGroup=${pgId} for ${key}`)
      useTabsStore.setState((s) => ({ tabs: [...s.tabs, tab] }))
      useTabsStore.getState().saveLayoutForWorkspace(projectId, workspaceId)
    },
    onRemoved: (event: SessionRemovedEvent) => {
      const pgId = event.pane_group_id
      if (!pgId || !event.agent_name.startsWith('tab-')) return
      if (useTabsStore.getState().activeWorkspaceKey !== key) return
      const state = useTabsStore.getState()

      const droppedFromMain = state.tabs.filter(
        (t) => !tabIsDropCandidateForSessionRemoval(t, pgId),
      )
      const newExtraGroups = state.extraGroups.map((g) => ({
        tabs: g.tabs.filter((t) => !tabIsDropCandidateForSessionRemoval(t, pgId)),
        activeTabId: g.activeTabId,
      }))
      const mainDelta = state.tabs.length - droppedFromMain.length
      const extraDelta = state.extraGroups.reduce(
        (n, g, i) => n + (g.tabs.length - newExtraGroups[i].tabs.length),
        0,
      )
      if (mainDelta === 0 && extraDelta === 0) return

      // Re-derive activeTabId if we dropped the active one.
      let newActiveId = state.activeTabId
      if (newActiveId && !droppedFromMain.find((t) => t.id === newActiveId)) {
        // Was it in extraGroups? If still present there, keep it.
        const stillExists = newExtraGroups.some((g) =>
          g.tabs.find((t) => t.id === newActiveId),
        )
        if (!stillExists) {
          newActiveId = droppedFromMain[0]?.id ?? null
        }
      }
      console.warn(`[tabs] session_removed push — dropped paneGroup=${pgId} for ${key}`)
      useTabsStore.setState({
        tabs: droppedFromMain,
        activeTabId: newActiveId,
        extraGroups: newExtraGroups.map((g) => ({
          tabs: g.tabs,
          activeTabId: g.tabs.find((t) => t.id === g.activeTabId)
            ? g.activeTabId
            : g.tabs[0]?.id ?? null,
        })),
      })
      useTabsStore.getState().saveLayoutForWorkspace(projectId, workspaceId)
    },
    onHello: () => {
      // First message after (re)connect. No-op on initial connect
      // because the renderer was just reconciled. On a reconnect
      // after a transient drop the renderer could have missed an
      // emit window — defensively re-fetch the daemon's snapshot
      // and re-run the orphan-adoption pass.
      if (useTabsStore.getState().activeWorkspaceKey !== key) return
      void (async () => {
        const sessions = await fetchDaemonSessions(cwd)
        if (sessions === null) return
        if (useTabsStore.getState().activeWorkspaceKey !== key) return
        const state = useTabsStore.getState()
        const adopted: Tab[] = []
        for (const s of sessions) {
          if (!s.isV2 || !s.agentName.startsWith('tab-')) continue
          const pgId = s.agentName.slice(4)
          if (isPaneGroupSurfaced(state, pgId)) continue
          adopted.push(
            buildAdoptedTerminalTab({
              paneGroupId: pgId,
              cwd: s.cwd || cwd,
              command: s.command ?? undefined,
              args: s.args.length > 0 ? s.args : undefined,
              sessionId: s.sessionId,
            }),
          )
        }
        if (adopted.length > 0) {
          console.warn(`[tabs] hello reconcile — adopted ${adopted.length} orphan(s) for ${key}`)
          useTabsStore.setState((s) => ({ tabs: [...s.tabs, ...adopted] }))
          useTabsStore.getState().saveLayoutForWorkspace(projectId, workspaceId)
        }
      })()
    },
  })
}

/** Close the active workspace's WS subscription and forget the key.
 *  Idempotent — safe to call when no subscription is active. */
function tearDownActiveWorkspaceSubscription(): void {
  if (activeSessionEventsUnsub) {
    try {
      activeSessionEventsUnsub()
    } catch (err) {
      console.warn('[tabs] failed to tear down session events sub:', err)
    }
    activeSessionEventsUnsub = null
    activeSessionEventsKey = null
  }
  if (activeTabEventsUnsub) {
    try {
      activeTabEventsUnsub()
    } catch (err) {
      console.warn('[tabs] failed to tear down tab events sub:', err)
    }
    activeTabEventsUnsub = null
  }
}

// ── 0.39.39 (#676/#677) tab-title snapshot + remote-reorder re-fetch ───────

interface DaemonTabTitle {
  projectId: string
  tabId: string
  title: string
}

/** Fetch the daemon-canonical tab titles for a project and apply them to
 *  the surfaced tabs (hydrate labels from the daemon on workspace load +
 *  on reconnect). No-op for tab ids not currently surfaced. Best-effort:
 *  swallows the route-absent / transient case (the local layout label
 *  remains the fallback). */
async function applyTabTitlesSnapshot(projectId: string): Promise<void> {
  try {
    const titles = await daemonCliGet<DaemonTabTitle[]>('workspace/tab-titles', {
      project_id: projectId,
    })
    if (!Array.isArray(titles)) return
    const store = useTabsStore.getState()
    for (const t of titles) {
      if (t && typeof t.tabId === 'string' && typeof t.title === 'string') {
        store.applyDaemonTabTitle(t.tabId, t.title)
      }
    }
  } catch (err) {
    console.debug('[tabs] tab-titles snapshot skipped:', err)
  }
}

/** Stable identity for matching a tab across a layout save/restore round-trip.
 *  `paneGroupId`s are preserved on the main group (restoreLayout "Reuse the
 *  saved ID", tabs.ts:2839), but the tab's own `id` is re-minted on restore.
 *  So a tab's signature is its sorted, joined paneGroupId set — this matches a
 *  serialized tab to the live `Tab` that owns the same panes regardless of the
 *  re-minted tab id. */
function serializedTabSignature(t: SerializedTab): string {
  const pgIds = t.paneGroups ? Object.keys(t.paneGroups) : []
  return pgIds.slice().sort().join(' ')
}

function liveTabSignature(t: Tab): string {
  return Array.from(t.paneGroups.keys()).slice().sort().join(' ')
}

/** per-client-view-state.md (Phase 2) — resolve the active (selected) tab for a
 *  freshly-restored main-group tab list from the PER-CLIENT selected-tabs store.
 *
 *  The store records the user's selection as a paneGroup SIGNATURE
 *  ({@link liveTabSignature}) rather than a tab id, because `restoreLayout`
 *  re-mints tab ids on every restore while the main group's paneGroupIds are
 *  preserved. We look up `getSelectedTab(projectId, workspaceId)` and match it
 *  against the restored tabs by signature.
 *
 *  Fallbacks (in order): no project/workspace ids threaded yet → first tab;
 *  no saved selection (brand-new client) → first tab; saved selection's tab no
 *  longer exists (closed on a peer) → first tab. The first tab is the system
 *  Chat tab on cold boot, so #658 pinned-chat-as-default is preserved. */
function resolveRestoredSelection(
  state: { activeProjectId: string | null; activeWorkspaceId: string | null },
  restoredTabs: Tab[],
): string | null {
  if (restoredTabs.length === 0) return null
  const firstId = restoredTabs[0].id
  const { activeProjectId, activeWorkspaceId } = state
  if (!activeProjectId || !activeWorkspaceId) return firstId
  const savedSig = getSelectedTab(activeProjectId, activeWorkspaceId)
  if (!savedSig) return firstId
  const match = restoredTabs.find((t) => liveTabSignature(t) === savedSig)
  return match ? match.id : firstId
}

/** per-client-view-state.md (Phase 2) — persist the user's PRIMARY (group-0)
 *  selection into the per-client selected-tabs store, keyed by the active
 *  workspace and recorded as the selected tab's paneGroup signature (stable
 *  across a restore's tab-id re-mint). No-op when the workspace ids aren't
 *  threaded yet or the tab isn't a live group-0 tab (e.g. a system/transient
 *  id that isn't in `state.tabs`). `state` is the pre-mutation snapshot, which
 *  still contains the selected tab object. */
function persistGroup0Selection(
  state: { activeProjectId: string | null; activeWorkspaceId: string | null; tabs: Tab[] },
  tabId: string,
): void {
  const { activeProjectId, activeWorkspaceId } = state
  if (!activeProjectId || !activeWorkspaceId) return
  const tab = state.tabs.find((t) => t.id === tabId)
  if (!tab) return
  setSelectedTab(activeProjectId, activeWorkspaceId, liveTabSignature(tab))
}

/** If the incoming `layout.tabs` is a PURE REORDER of the live main `tabs`
 *  (same SET of tabs by signature, same count, just a different order), adopt
 *  the new order IN PLACE — reusing the existing `Tab` objects so each tab's
 *  `id` (the React key) and live `paneGroups`/terminals survive. React then
 *  MOVES the existing terminal components instead of unmounting+remounting
 *  them (no respawn, no flicker, no pinned-chat `ensure-pinned-chat` re-fire).
 *
 *  Returns `true` when it handled the change in place; `false` when the SET
 *  differs (a tab was added/removed on the peer) and the caller must fall back
 *  to the full `restoreLayout` rebuild.
 *
 *  Only the main `tabs` group is eligible: when the live workspace has split
 *  `extraGroups`, the order spans multiple groups and we defer to the full
 *  rebuild (returns false). All state mutation here runs under the caller's
 *  `withLayoutSaveSuppressed` window so it never echoes a save. */
function tryReorderTabsInPlace(key: string, layout: SerializedLayout): boolean {
  const state = useTabsStore.getState()

  // Split layouts: order spans multiple groups — defer to the full rebuild.
  if (state.extraGroups.length > 0) return false
  if (layout.extraGroups && layout.extraGroups.length > 0) return false

  const liveTabs = state.tabs
  const newOrder = layout.tabs
  if (newOrder.length !== liveTabs.length) return false
  if (liveTabs.length === 0) return false

  // Build signature -> live Tab. If two live tabs share a signature (no panes,
  // or an unexpected collision) we can't match 1:1 deterministically — bail to
  // the full rebuild rather than risk a mis-order.
  const bySignature = new Map<string, Tab>()
  for (const t of liveTabs) {
    const sig = liveTabSignature(t)
    if (bySignature.has(sig)) return false
    bySignature.set(sig, t)
  }

  // Map each entry in the new order 1:1 to a live tab by signature. Any miss
  // (a tab added/removed on the peer) or duplicate consumption means the SET
  // differs → fall back.
  const reordered: Tab[] = []
  const consumed = new Set<Tab>()
  for (const st of newOrder) {
    const sig = serializedTabSignature(st)
    const live = bySignature.get(sig)
    if (!live || consumed.has(live)) return false
    consumed.add(live)
    reordered.push(live)
  }
  if (reordered.length !== liveTabs.length) return false

  // per-client-view-state.md (Phase 3) — adoption NEVER moves this client's
  // selection. The peer's reorder is a PURE permutation of the same tab ids,
  // so the locally-selected `state.activeTabId` is still valid and we KEEP it.
  // (The old `layout.activeTabId` override that re-pointed selection at the
  // peer's serialized selection is gone — that was the multi-client hijack.)
  let nextActiveId = state.activeTabId
  // If the previously-active id somehow isn't in the (unchanged) set, anchor to
  // the first tab of the new order.
  if (!nextActiveId || !reordered.some((t) => t.id === nextActiveId)) {
    nextActiveId = reordered[0]?.id ?? null
  }

  useTabsStore.setState((s) => ({
    tabs: reordered,
    activeTabId: nextActiveId,
    workspaceLayouts: { ...s.workspaceLayouts, [key]: layout },
  }))
  console.warn(`[tabs] adopted remote tab-order (in place) for ${key}`)
  return true
}

/** Re-fetch the canonical layout after a remote client reordered ahead of
 *  our base revision (#677.3) and adopt it, so we don't silently keep a
 *  stale local order. Guards on the workspace still being active. */
async function refetchLayoutForRemoteReorder(
  key: string,
  projectId: string,
  workspaceId: string,
  cwd: string,
): Promise<void> {
  try {
    const json = await daemonCliGet<string | null>('workspace-layouts/load', {
      project_id: projectId,
      workspace_id: workspaceId,
    })
    if (useTabsStore.getState().activeWorkspaceKey !== key) return
    if (!json) return
    const layout = JSON.parse(json) as SerializedLayout
    if (!layout.tabs || layout.tabs.length === 0) return
    // Adopt the peer's layout SILENTLY. Whether we reorder in place or fall
    // back to `restoreLayout`, the mutation runs under suppression so the
    // autosave subscription (which fires synchronously inside the store's
    // `set(...)`) does NOT echo a `workspace-layouts/save` back to the daemon
    // — the echo that would re-broadcast `TabOrderChanged` and make two clients
    // ping-pong forever (#676/#677 0.39.39 regression). The base revision was
    // already advanced via `recordLayoutRevision` at the call site, so a
    // duplicate broadcast at this revision stays a no-op.
    //
    // Also cancel any pending pre-adoption autosave: its debounced timer fires
    // ~1s later (after suppression has lowered) and would serialize CURRENT
    // (= the just-adopted) state, re-emitting the echo we just suppressed. The
    // peer's canonical layout supersedes that stale local write (LWW — it has a
    // higher revision), so dropping the pending save is correct, not lossy.
    cancelPendingLayoutSave()
    withLayoutSaveSuppressed(() => {
      // #2 (companion to #1): if the change is a PURE REORDER of the current
      // live tabs (same set, different order), permute the EXISTING `Tab`
      // objects in place — reusing each tab's `id` (the React key) and live
      // `paneGroups`/terminals. React moves the terminal components without
      // unmounting, so a cross-client reorder is visual-only (no terminal
      // reload, no pinned-chat re-ensure). Only when the tab SET actually
      // differs (add/remove on the peer) do we fall back to the full
      // `restoreLayout` rebuild (which re-mints tab/pane ids → remount).
      if (tryReorderTabsInPlace(key, layout)) return
      // per-client-view-state.md (Phase 3) — the rebuild fallback re-anchors
      // selection via `restoreLayout` → `resolveRestoredSelection`, which
      // sources THIS client's selection from the per-client store (matched by
      // paneGroup signature) and falls back to the first tab. It does NOT read
      // the peer's serialized `activeTabId` (now omitted), so a peer's tab-set
      // change can never hijack this client's selected tab.
      useTabsStore.setState((s) => ({ workspaceLayouts: { ...s.workspaceLayouts, [key]: layout } }))
      useTabsStore.getState().restoreLayout(layout, cwd)
      console.warn(`[tabs] adopted remote tab-order reorder (rebuild) for ${key}`)
    })
  } catch (err) {
    console.warn('[tabs] remote-reorder re-fetch failed:', err)
  }
}

// Export for tests / external introspection. Internal callers should
// use the `subscribeForActiveWorkspace` flow above.
export function __getActiveSessionEventsKey(): string | null {
  return activeSessionEventsKey
}

/** Test-only: drive the in-place reorder adoption directly (the path the
 *  remote-reorder re-fetch takes for a pure permutation). Exercised by
 *  selected-tabs.test.ts to prove adoption never moves local selection. */
export function __tryReorderTabsInPlaceForTests(key: string, layout: SerializedLayout): boolean {
  return tryReorderTabsInPlace(key, layout)
}

// ── Legacy format conversion ─────────────────────────────────────────────

interface LegacySerializedPaneData {
  type: 'terminal' | 'file-viewer'
  cwd?: string
  command?: string
  args?: string[]
  filePath?: string
  pinned?: boolean
}

/**
 * Convert legacy serialized panes (flat Record<string, PaneData>)
 * into the new SerializedPaneGroup format for backward-compat restore.
 * Emits v1 terminal items; the v1→v2 migration pass in `restoreLayout`
 * normalises them after.
 */
function convertLegacyPanes(
  panes: Record<string, LegacySerializedPaneData> | undefined
): Record<string, SerializedPaneGroup> {
  if (!panes) return {}

  const result: Record<string, SerializedPaneGroup> = {}
  for (const [id, pane] of Object.entries(panes)) {
    let item: SerializedItem
    if (pane.type === 'terminal') {
      const t: SerializedTerminalItemV1 = {
        id,
        type: 'terminal',
        cwd: pane.cwd,
        command: pane.command,
        args: pane.args,
      }
      item = t
    } else {
      const f: SerializedFileViewerItem = {
        id,
        type: 'file-viewer',
        filePath: pane.filePath,
        pinned: pane.pinned,
      }
      item = f
    }
    result[id] = {
      id,
      items: [item],
      activeItemIndex: 0,
    }
  }
  return result
}

// ── Workspace ops event payloads ─────────────────────────────────────────

interface WsSplitPanePayload {
  tabId: string
  paneId: string
  direction: 'horizontal' | 'vertical'
}

interface WsClosePanePayload {
  tabId: string
  paneId: string
}

interface WsOpenDocumentPayload {
  tabId: string
  paneId: string
  filePath: string
}

interface WsOpenTerminalPayload {
  tabId: string
  paneId: string
  cwd: string
  command?: string
}

interface WsNewTabPayload {
  cwd: string
}

interface WsCloseTabPayload {
  tabId: string
}

interface LayoutLeafDescriptor {
  type: 'document' | 'terminal'
  path?: string
  command?: string
  cwd?: string
}

interface LayoutBranchDescriptor {
  direction: 'horizontal' | 'vertical'
  children: [LayoutDescriptor, LayoutDescriptor]
  splitPercentage?: number
}

type LayoutDescriptor = LayoutLeafDescriptor | LayoutBranchDescriptor

function isLayoutBranch(d: LayoutDescriptor): d is LayoutBranchDescriptor {
  return 'children' in d && 'direction' in d
}

/**
 * Convert a backend direction string to MosaicDirection.
 * "horizontal" -> split side-by-side -> 'row'
 * "vertical"   -> split top/bottom  -> 'column'
 */
function toMosaicDirection(dir: 'horizontal' | 'vertical'): MosaicDirection {
  return dir === 'horizontal' ? 'row' : 'column'
}

/**
 * Recursively build a MosaicNode tree and collect PaneGroups
 * from a layout descriptor.
 */
function buildMosaicFromDescriptor(
  descriptor: LayoutDescriptor,
  paneGroups: Map<string, PaneGroup>
): MosaicNode<string> {
  if (isLayoutBranch(descriptor)) {
    const first = buildMosaicFromDescriptor(descriptor.children[0], paneGroups)
    const second = buildMosaicFromDescriptor(descriptor.children[1], paneGroups)
    return {
      direction: toMosaicDirection(descriptor.direction),
      first,
      second,
      splitPercentage: descriptor.splitPercentage ?? 50
    }
  }

  // Leaf node
  const paneGroupId = crypto.randomUUID()

  if (descriptor.type === 'document') {
    const pg = makeFileViewerPaneGroup(paneGroupId, descriptor.path ?? '', true)
    paneGroups.set(paneGroupId, pg)
  } else {
    const pg = makeTerminalPaneGroup(paneGroupId, descriptor.cwd ?? '~', {
      command: descriptor.command,
    })
    paneGroups.set(paneGroupId, pg)
  }

  return paneGroupId
}

// ── Wire up Tauri event listeners for workspace operations ───────────────

async function initWorkspaceOpsListeners(): Promise<void> {
  try {
    const { listen } = await import('@tauri-apps/api/event')
    const store = useTabsStore

    // workspace:split-pane -> split an existing pane in a tab
    //
    // GH#639: every `listen()` returns a promise that REJECTS in the
    // headless test env (no Tauri window). Awaiting each one funnels
    // those rejections into THIS function's returned promise (caught by
    // the surrounding try/catch + the call-site `.catch()`), instead of
    // surfacing ~11 unhandled rejections that flip vitest's exit code.
    await listen<WsSplitPanePayload>('workspace:split-pane', (event) => {
      const { tabId, paneId, direction } = event.payload
      const newPaneGroupId = crypto.randomUUID()
      const newPane: TerminalPaneData = {
        type: 'terminal',
        terminalId: newPaneGroupId,
        cwd: '~'
      }
      store.getState().splitPane(
        tabId,
        paneId,
        newPaneGroupId,
        newPane,
        toMosaicDirection(direction)
      )
    })

    // workspace:close-pane -> remove a paneGroup from a tab
    await listen<WsClosePanePayload>('workspace:close-pane', (event) => {
      const { tabId, paneId } = event.payload
      store.getState().removePaneFromTab(tabId, paneId)
    })

    // workspace:open-document -> add a file-viewer item to the paneGroup
    await listen<WsOpenDocumentPayload>('workspace:open-document', (event) => {
      const { tabId, paneId, filePath } = event.payload
      const state = store.getState()
      const tab = state.tabs.find((t) => t.id === tabId)
      if (!tab) return

      if (tab.paneGroups.has(paneId)) {
        // Add a file-viewer item to the existing paneGroup
        const newItem: Item = {
          id: crypto.randomUUID(),
          type: 'file-viewer',
          data: { filePath },
          pinned: true,
        }
        state.addItemToPaneGroup(tabId, paneId, newItem)
      } else {
        // PaneGroup doesn't exist — use the store's openFileInPane
        state.openFileInPane(tabId, filePath)
      }
    })

    // workspace:open-terminal -> add a terminal item or create a new paneGroup
    await listen<WsOpenTerminalPayload>('workspace:open-terminal', (event) => {
      const { tabId, paneId, cwd, command } = event.payload
      const state = store.getState()
      const tab = state.tabs.find((t) => t.id === tabId)
      if (!tab) return

      if (tab.paneGroups.has(paneId)) {
        // Add a terminal item to the existing paneGroup
        const newItem: Item = {
          id: crypto.randomUUID(),
          type: 'terminal',
          data: {
            terminalId: paneId,
            cwd,
            command,
          },
        }
        state.addItemToPaneGroup(tabId, paneId, newItem)
      } else {
        // PaneGroup doesn't exist — create it and add to mosaic
        const pg = makeTerminalPaneGroup(paneId, cwd, { command })
        state.addPaneToTab(tabId, paneId, {
          type: 'terminal',
          terminalId: paneId,
          cwd,
          command,
        })

        const existingLeaf = getFirstLeaf(tab.mosaicTree)
        if (existingLeaf && tab.mosaicTree !== null) {
          const splitNode: MosaicNode<string> = {
            direction: 'column',
            first: existingLeaf,
            second: paneId,
            splitPercentage: 50
          }
          const newTree = replaceInTree(tab.mosaicTree, existingLeaf, splitNode)
          if (newTree) {
            state.updateMosaicTree(tabId, newTree)
          }
        } else {
          state.updateMosaicTree(tabId, paneId)
        }
      }
    })

    // workspace:new-tab -> create a new tab
    await listen<WsNewTabPayload>('workspace:new-tab', (event) => {
      const { cwd } = event.payload
      store.getState().addTab(cwd)
    })

    // workspace:close-tab -> close a tab
    await listen<WsCloseTabPayload>('workspace:close-tab', (event) => {
      const { tabId } = event.payload
      store.getState().removeTab(tabId)
    })

    // workspace:arrange -> build a full layout from a descriptor
    await listen<LayoutDescriptor>('workspace:arrange', (event) => {
      const descriptor = event.payload
      const state = store.getState()

      const paneGroups = new Map<string, PaneGroup>()
      const mosaicTree = buildMosaicFromDescriptor(descriptor, paneGroups)

      tabCounter++
      const tabId = crypto.randomUUID()

      // Derive tab title from the first paneGroup
      let title = `Layout ${tabCounter}`
      for (const pg of paneGroups.values()) {
        const firstItem = pg.items[0]
        if (firstItem?.type === 'file-viewer') {
          const d = firstItem.data as FileViewerItemData
          title = d.filePath.split('/').pop() ?? title
          break
        }
      }

      const tab: Tab = {
        id: tabId,
        title,
        mosaicTree,
        paneGroups
      }

      // Add the arranged tab and make it active
      store.setState((s) => ({
        tabs: [...s.tabs, tab],
        activeTabId: tabId
      }))
    })
  } catch {
    // Tauri API not available (e.g. in tests)
  }
}

// Initialize listeners on import. GH#639: swallow the rejection the
// awaited `listen()` calls produce in the headless test env so it never
// escapes as an unhandled rejection (which flips vitest's exit code).
void initWorkspaceOpsListeners().catch(() => {})

// Load persisted workspace sessions from DB on import
useTabsStore.getState().loadWorkspaceSessionsFromDb()

// Phase 2.5 fix (finding #547): retry the layouts load on daemon
// (re)connect when the import-time fetch lost a race with the
// daemon's startup. Without this, an early-boot failure left
// `workspaceLayouts` empty for the session, and the next workspace
// activation built a fresh default layout that auto-save then
// persisted over the real one.
onDaemonConnected(() => {
  if (hasLoadedWorkspaceSessions) return
  useTabsStore.getState().loadWorkspaceSessionsFromDb()
})

// #625 — host-switch reset of per-machine workspace-session state.
//
// `backgroundWorkspaces` / `workspaceLayouts` / `activeWorkspaceKey` are
// keyed by LOCAL project/workspace IDs and were loaded ONCE at import via
// `loadWorkspaceSessionsFromDb()` above. They (plus the module-level
// `hasLoadedWorkspaceSessions` gate) are module singletons — they survive
// the `<App key={hostKey}>` remount on a host switch. Without this reset,
// after connecting to a REMOTE daemon the IconRail active section + Active
// Bar rule 4 keep showing the LOCAL machine's stashed workspaces.
//
// On a real host CHANGE: clear the local-keyed state, drop the load gate,
// and reload from the NEW host's daemon. `loadWorkspaceSessionsFromDb`
// goes through the host-aware `daemonCli*` layer (reads `activeHost` at
// call time), and `onActiveHostChange` fires AFTER `activeHost` has
// flipped, so the reload targets the new host. Exported as a named fn so
// a focused test can invoke it without a live store subscription.
export function __resetWorkspaceSessionsForHostSwitch(): void {
  hasLoadedWorkspaceSessions = false
  // Cancel any pending layout-save debounce so a queued write keyed to the
  // OLD host's workspace can't fire its `daemonCli*` persist against the
  // NEW host after the flip.
  if (persistDebounceTimer) {
    clearTimeout(persistDebounceTimer)
    persistDebounceTimer = null
  }
  // Tear down the active workspace's session-events WS to the OLD host so
  // we don't keep a live subscription open against a daemon we've left.
  // The new host's subscription is established when the next workspace is
  // restored (subscribeForActiveWorkspace). Idempotent when no sub is open.
  tearDownActiveWorkspaceSubscription()
  useTabsStore.setState({
    backgroundWorkspaces: {},
    workspaceLayouts: {},
    activeWorkspaceKey: null,
    activeProjectId: null,
    activeWorkspaceId: null,
  })
  // per-client-view-state.md (Phase 2) — selection is per-machine; the NEW
  // host has its own workspaces/selections. Clear this machine's stashed
  // selection map so a stale local-host selection can't be matched against a
  // remote-host workspace's tabs. (Mirrors the other per-machine resets.)
  resetSelectedTabs()
  void useTabsStore.getState().loadWorkspaceSessionsFromDb()
}

onActiveHostChange(() => {
  __resetWorkspaceSessionsForHostSwitch()
})

/** Test-only read of the module-level load gate (#625 reset assertions). */
export function __hasLoadedWorkspaceSessionsForTests(): boolean {
  return hasLoadedWorkspaceSessions
}

// Auto-save: subscribe to tab structure changes and persist to DB (debounced).
// This provides crash resilience — lose at most ~1 second of tab changes.
//
// per-client-view-state.md (Phase 1) — `activeTabId` is NO LONGER part of the
// canonical layout, so a pure SELECTION change is intentionally NOT an autosave
// trigger: the serialized structure would be byte-identical (a wasted SQL
// write) AND a save bumps the layout revision + re-broadcasts `TabOrderChanged`
// to peers — i.e. clicking a tab would needlessly churn every other client.
// Selection persistence is handled locally by the per-client selected-tabs
// store (`setActiveTabInGroup`/`setActiveTab`), not this canonical save path.
useTabsStore.subscribe(
  (state, prevState) => {
    // Only trigger on meaningful tab structure changes (not backgroundWorkspaces
    // swaps, and not pure selection changes — see note above).
    if (
      state.tabs !== prevState.tabs ||
      state.extraGroups !== prevState.extraGroups ||
      state.splitCount !== prevState.splitCount ||
      state.activeGroupIndex !== prevState.activeGroupIndex
    ) {
      if (state.activeWorkspaceKey && state.tabs.length > 0) {
        state.persistActiveWorkspace()
      }
    }
  }
)
