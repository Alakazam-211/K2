import { create, type StoreApi, type UseBoundStore } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliGetText, daemonCliPost, RecoveringError } from '@/lib/daemon-cli'
import { jittered } from '@/lib/backoff'
import { agentDisplayName } from '@/lib/workspace-agent'
import { isBuiltinAgentType } from '@/lib/agent-type'
import { asArray } from '@/lib/as-array'
import {
  adoptRestoredTab,
  adoptTabTitle,
  collectStoreTabs,
  conversationIdFromTab,
  conversationIdFromTerminal,
  findChatSessionInTab,
  pickConversationId,
  rememberLiveNamedChatTitles,
  rememberTabTitleSnapshot,
  restampSessionTabsFromChatList,
} from '@/lib/chat-session-tab'
import { terminalKill } from '@/lib/terminal-daemon'
import type { MosaicNode, MosaicDirection } from 'react-mosaic-component'
import { RESUMABLE_CLI_TOOLS } from '@shared/constants'
import { resolveAgentCommand, type AgentPresetLike } from '@/lib/agent-resolve'
import { useSettingsStore } from '@/stores/settings'
import { useTerminalSettingsStore, type TerminalRenderer } from '@/stores/terminal-settings'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { withCliTokenQuery, withDaemonFetch } from '@/web/session-token'
import { webFeatures } from '@/web/features'
import { useHeartbeatSessionsStore, type HeartbeatEntry } from '@/stores/heartbeat-sessions'
// Phase 2.5 fix (finding #547) — retry the workspace-layouts load
// when the daemon comes online after a slow boot.
import { onDaemonConnected } from '@/lib/daemon-reconnect'
// #625 — the primary room resets its per-machine workspace-session state on
// a host switch; its scope's `connectionKey` guards in-flight layout loads so
// a response from the PREVIOUS host can never land after a switch.
import { onActiveHostChange } from '@/stores/connect-host'
// per-client-view-state.md — the user's SELECTED tab is per-client view
// state, sourced from this local store (never the shared layout's leaked
// `activeTabId`). Module-level fns avoid a React subscription in this store.
import { getSelectedTab, setSelectedTab, resetSelectedTabs } from '@/stores/selected-tabs'
import {
  subscribeToWorkspaceSessionEvents,
  subscribeToWorkspaceTabEvents,
  onSessionAddedApp,
  onSessionRemovedApp,
  onAppHello,
  onOpenUrl,
  onceRecovered,
  type SessionAddedEvent,
  type SessionRemovedEvent,
  type TabTitleChangedEvent,
  type TabOrderChangedEvent,
  type UnsubscribeFn,
} from '@/stores/session-events'
import { paintableBrowserIcon } from '@/lib/browser-tab-icon'
import { takeSessionRemoved } from '@/lib/sidecar-refresh-tab'
import {
  EMPTY_SERIALIZED_LAYOUT,
  canonicalLayoutJson,
  collapseEmptyLeadingColumns,
  mergeSerializedLayouts,
} from '@/lib/layout-merge'
import { assertScopeMayPost, primaryScope, type ServerScope } from '@/kessel/server-scope'

/** Project path → id (+ primary workspace) for host-session tab routing.
 *  Scout sales pilot: api- SessionAdded must park under the event's
 *  workspace, not the focused strip. Lazy ref avoids projects↔tabs cycle. */
export type ProjectPathEntry = {
  id: string
  path: string
  /** First workspace id by tabOrder (layout key `${projectId}:${workspaceId}`). */
  primaryWorkspaceId: string | null
  /** When true, do not auto-adopt API host-session / sandbox tabs. */
  hideApiSessions?: boolean
}
function normalizeFsPath(p: string): string {
  if (!p) return ''
  const t = p.replace(/\/+$/, '')
  return t.length === 0 ? '/' : t
}
/** Resolve a project in `list` whose path equals (or is a parent of) `cwd`.
 *  `list` is always ONE room's own project list (MS3): a path is only an
 *  identity inside the server it came from. */
function findProjectForPathIn(list: ProjectPathEntry[], cwd: string): ProjectPathEntry | null {
  const n = normalizeFsPath(cwd)
  if (!n || n === '/') return null
  // Prefer exact path match; then longest registered path that is a parent
  // of cwd (host-session cwd is the workspace root for F1 host cells).
  let best: ProjectPathEntry | null = null
  for (const p of list) {
    const pp = normalizeFsPath(p.path)
    if (!pp) continue
    if (pp === n) return p
    if (n.startsWith(pp + '/')) {
      if (!best || normalizeFsPath(best.path).length < pp.length) best = p
    }
  }
  return best
}

// ── One tabs store per room (Home M3, prd-home-multi-server-client MS14/MS15) ──
//
// A ROOM is one workspace shown on one server: `(hostKey, projectId,
// workspaceId)`. Each room has its own tabs store instance, built by
// `createTabsStore(binding)`. Everything that used to be module state here
// (layout revisions, acked layouts, the save lanes, the autosave debounce
// and retry timers, the workspace socket handles, the load gate, the tab
// counter) lives in that instance's closure, and every daemon call the
// instance makes goes to the binding's `ServerScope` — captured at creation,
// never passed per call (vs-live MS65), so the store cannot express a write
// to another server.
//
//   - The PRIMARY room (`useTabsStore`, below) is the window's own room. It
//     follows the window's selected workspace, stashes the others, loads
//     every layout (`load-all`), and resets on a server switch — exactly
//     what the module singleton did before M3.
//   - A PINNED room holds ONE workspace on one scope. It never stashes, never
//     runs `load-all`, never listens to `onActiveHostChange`, and has its own
//     autosave subscription. `room.dispose()` flushes its based save and
//     closes its sockets.
//
// The cross-store getters the projects / presets stores used to inject into
// module refs are now the binding's `deps`. The primary room's deps are the
// registered ones (`register*` below); a pinned room's deps are its own
// server's data, passed at creation. A path lookup only ever searches the
// room's own project list (MS3).

/** What a tabs store reads from the stores around it. */
export interface TabsRoomDeps {
  /** The room's own project list, for path → (project, workspace) lookups. */
  projectsPathIndex: () => ProjectPathEntry[]
  /** The room's selected project, when the tabs store has no workspace key. */
  activeProjectId: () => string | null
  /** The open/attach ⇒ activate gesture (#672), on the room's server. */
  activateProject: (projectId: string) => void
  /** A project's per-workspace default agent (NULL ⇒ inherit global). */
  projectDefaultAgent: (projectId: string) => string | undefined
  /** Agent presets for `launchDefaultAgent`. */
  presets: () => { presets: any[] } | null
  /** Heartbeat rows the close-as-minimize cross-reference reads. */
  heartbeatEntries: () => HeartbeatEntry[]
}

/** The one workspace a pinned room shows. */
export interface TabsRoomWorkspace {
  projectId: string
  workspaceId: string
  /** The workspace path ON THAT SERVER (cwd of its terminals). */
  path: string
}

export interface TabsRoomBinding {
  /** The server every request, socket and save of this room goes to. */
  scope: ServerScope
  /** `null` = the primary room (follows the window's selected workspace).
   *  Otherwise the one workspace a pinned room holds. */
  workspace: TabsRoomWorkspace | null
  deps: TabsRoomDeps
  /** MS67 — may this room call THIS computer's Tauri commands
   *  (`k2so_agents_list`, …)? The primary room keeps today's behaviour
   *  (true); a pinned room is true only when its scope is `local`. */
  localCommands: boolean
}

// Primary-room deps: the stores around tabs register their getters here
// (projects.ts / presets.ts import tabs.ts, so they cannot be imported back).
// Unregistered (a vitest unit that never imports projects.ts) they read as
// "nothing": no project list, no foreground, a no-op activate, the global
// default agent, no presets.
const primaryDeps: TabsRoomDeps = {
  projectsPathIndex: () => [],
  activeProjectId: () => null,
  activateProject: () => {},
  projectDefaultAgent: () => undefined,
  presets: () => null,
  heartbeatEntries: () => useHeartbeatSessionsStore.getState().active,
}

/** presets.ts registers its store (presets → tabs → presets cycle). */
export function registerPresetsStore(getter: () => { presets: any[] }): void {
  primaryDeps.presets = getter
}

/** #657 — projects.ts registers the window's `activeProjectId` reader. */
export function registerActiveProjectIdGetter(getter: () => string | null): void {
  primaryDeps.activeProjectId = getter
}

/** Host-session tab routing (Scout sales pilot): projects.ts registers the
 *  window server's project list so api- SessionAdded parks under the
 *  event's workspace, not the focused strip. */
export function registerProjectsPathIndex(getter: () => ProjectPathEntry[]): void {
  primaryDeps.projectsPathIndex = getter
}

/** #672 — projects.ts registers the canonical open/attach ⇒ activate
 *  gesture (PRD §4.3.1). The chat-surfacing chokepoint
 *  (`subscribeForActiveWorkspace`) calls it so EVERY path that surfaces a
 *  workspace chat is an activation — the safety property daemon-side
 *  Active-only reaping leans on. */
export function registerActivateProject(fn: (projectId: string) => void): void {
  primaryDeps.activateProject = fn
}

/** Agent-degeneralization S1 — projects.ts registers a project's
 *  per-workspace default agent reader. */
export function registerProjectDefaultAgentGetter(
  getter: (projectId: string) => string | undefined,
): void {
  primaryDeps.projectDefaultAgent = getter
}

// ── Renderer reaping REMOVED (#672, daemon-canonical-active.md §4.5/§6) ──
//
// The daemon OWNS the Active set AND the grace-reap. The renderer no longer
// schedules or fires reaps for age-out/dismiss — it is a pure consumer of
// the canonical Active mirror (useActiveStore). `closeV2Session` stays —
// it's the deliberate tab-close path (closeTerminalForRenderer), not a
// reaper. The daemon reaper force-closes v2 sessions server-side.

// ── Unsaved-changes leave guard (save/discard/cancel on workspace switch) ──
//
// When the user switches AWAY from a workspace that has dirty file tabs, the
// projects store consults this guard. The guard (a UI promise that shows the
// "Unsaved changes" modal) is registered at the top level via
// `registerLeaveGuard`, mirroring the `registerActivateProject` indirection so
// the store can call UI code without importing React. If no guard is
// registered (e.g. startup, before the modal host mounts) the default is
// 'save' so the switch is NEVER blocked.
export type LeaveChoice = 'save' | 'discard' | 'cancel'
let _leaveGuardRef: ((leavingProjectId: string) => Promise<LeaveChoice>) | null = null
export function registerLeaveGuard(
  fn: (leavingProjectId: string) => Promise<LeaveChoice>,
): void {
  _leaveGuardRef = fn
}
export async function runLeaveGuard(leavingProjectId: string): Promise<LeaveChoice> {
  if (!_leaveGuardRef) return 'save'
  try {
    return await _leaveGuardRef(leavingProjectId)
  } catch {
    // A guard failure must never strand the user — fail safe to 'save'.
    return 'save'
  }
}

// ── Layout save pipeline (split view with two windows, V14–V19, V22) ─────
//
// Every writer of the shared layout goes through `submitLayoutSave`:
//   - saves for one workspace key run ONE AT A TIME. A save asked for while
//     another is in flight waits and then serializes the state it has then
//     (latest wins), so a window never races its own `baseRevision` (V17);
//   - a save whose JSON matches the layout the daemon last confirmed is
//     skipped (V19). That covers a remount's `setTabDirty` / file-viewer
//     scroll writes, a same-value browser stamp, and reconcile refreshes;
//   - a 409 `layout_revision_conflict` fetches the newer layout and merges
//     this window's change onto it (V18), or drops a background write whose
//     workspace is open somewhere newer. A conflict never goes to the blind
//     retry.
// A remote refetch runs in the same lane, so no save can go out with a base
// that a half-applied refetch is about to replace.

interface AckedLayout {
  /** Canonical JSON (layout-merge `canonicalLayoutJson`) for the V19 check. */
  canonical: string
  /** The layout itself, the base of a three-way merge. */
  layout: SerializedLayout
}

type LayoutConflictMode = 'merge' | 'drop'

interface LayoutSaveJob {
  projectId: string
  workspaceId: string
  /** Build the layout at send time. `null` = nothing to save now. */
  produce: () => SerializedLayout | null
  /** `merge` for the open workspace, `drop` for background / park writes. */
  conflict: LayoutConflictMode
  /** Send even when it matches the acked layout (v1→v2 migration). */
  force?: boolean
  /** Failures (not conflicts) re-arm the durable retry. */
  retry?: boolean
  label: string
}

interface LayoutLane {
  busy: boolean
  ops: Array<() => Promise<void>>
  queuedSave: LayoutSaveJob | null
  idle: Array<() => void>
}

function logLaneError(err: unknown): void {
  console.error('[tabs] layout lane op failed:', err)
}

function isLayoutRevisionConflict(err: unknown): boolean {
  return err instanceof Error && err.message.includes('layout_revision_conflict')
}

const LAYOUT_CONFLICT_MAX_ATTEMPTS = 3

interface FetchedLayout {
  layout: SerializedLayout | null
  /** `undefined` against an older daemon (no `with_revision`). */
  revision: number | undefined
}

/** POST `sessions/v2/close` to the ROOM's server (MS42: `closeV2Session`
 *  takes the room's scope — a close in B's room never reaches A). Exported
 *  for the pinned chat's Refresh (MS75), which closes its own session. */
export async function closeV2Session(
  scope: ServerScope,
  agentName: string,
  opts?: { clearIndex?: boolean; reason?: 'tab_close' },
): Promise<void> {
  // Home M4: a view-only room never closes a session on its server.
  assertScopeMayPost(scope, 'sessions/v2/close')
  try {
    const creds = await getDaemonWs(scope)
    const url = withCliTokenQuery(
      `${daemonHttpBase(creds)}/cli/sessions/v2/close`,
      creds.token,
    )
    const res = await fetch(
      url,
      withDaemonFetch({
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        // force: deliberate user tab-close — bypass the daemon's attached-
        // client close-guard (GH#22 reaper defense). The user closing the tab
        // IS the attached client; the guard only exists to stop the daemon
        // reaper, which never routes through here.
        body: JSON.stringify({
          agent_name: agentName,
          force: true,
          ...(opts?.clearIndex ? { clear_index: true } : {}),
          ...(opts?.reason ? { reason: opts.reason } : {}),
        }),
      }),
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
 * Session-selection flags stripped from preset args before appending an
 * explicit resume target (mirrors ChatHistory's SESSION_FLAGS_TO_STRIP).
 * Matters most for Pi: its `--resume` is an interactive picker, so a
 * leftover preset flag would shadow the appended `--session <uuid>`.
 */
const SESSION_FLAGS_TO_STRIP = new Set(['--resume', '-r', '--continue', '-c', '--session'])

/**
 * Resolve the user's enabled preset args for an agent command (e.g.
 * claude's --dangerously-skip-permissions). Lazy-imports the presets
 * store so module-load doesn't cascade through settings.ts (which calls
 * invoke and breaks vitest environments without a window). Strips any
 * pre-existing session-selection flag so callers can append their own
 * deterministically. Returns `[]` when no enabled preset is configured.
 */
async function resolvePresetArgs(command: string): Promise<string[]> {
  try {
    const { usePresetsStore } = await import('@/stores/presets')
    const presets = usePresetsStore.getState().presets
    const preset = presets.find((p) => p.command.split(/\s+/)[0] === command && p.enabled)
    if (!preset) return []
    const parts = preset.command.match(/(?:[^\s"']+|"[^"]*"|'[^']*')+/g) || []
    const cleaned = parts.map((p: string) => p.replace(/^["']|["']$/g, ''))
    return cleaned.slice(1).filter((a: string) => !SESSION_FLAGS_TO_STRIP.has(a))
  } catch {
    return []
  }
}

/**
 * Agent-degeneralization S4 — resolve the launch (command + args) that
 * resumes a DISCOVERED session in its own provider's grammar.
 *
 * The daemon's `chat/list` aggregator is the provider oracle: its rows
 * are parsed from each provider's on-disk session store, so a row's
 * presence both names the harness AND proves the conversation exists on
 * disk. Sessions not found there (or a null id) degrade to claude
 * grammar — the pre-S4 behavior and the only honest guess.
 */
async function resolveSessionResumeLaunch(
  scope: ServerScope,
  projectPath: string,
  sessionId: string | null,
): Promise<{ command: string; args: string[]; provider: string; listed: boolean }> {
  let provider = 'claude'
  let listed = false
  if (sessionId) {
    try {
      const rows = await daemonCliGet<
        Array<{ sessionId: string; provider?: string; archived?: boolean }>
      >(scope, 'chat/list', { project_path: projectPath })
      // Archived sessions are not resume-eligible until restored.
      const row = rows.find((r) => r.sessionId === sessionId && !r.archived)
      if (row) {
        provider = row.provider || 'claude'
        listed = true
      }
    } catch { /* daemon read failed — claude fallback */ }
  }
  // Invert RESUMABLE_CLI_TOOLS (command → tool) to provider → command.
  const entry = Object.entries(RESUMABLE_CLI_TOOLS)
    .find(([, tool]) => tool.provider === provider)
  const [command, tool] = entry ?? ['claude', RESUMABLE_CLI_TOOLS['claude']]
  let args: string[]
  if (tool.resumeSubcommand) {
    // Subcommand-style (codex): keep Settings → LLMs flags in front of
    // `resume <id>` (`codex --yolo resume <uuid>`).
    args = [...(await resolvePresetArgs(command)), tool.resumeSubcommand, sessionId ?? '']
  } else {
    args = [...(await resolvePresetArgs(command)), tool.resumeFlag ?? '--resume', sessionId ?? '']
  }
  return { command, args, provider, listed }
}

// ── Item Types ────────────────────────────────────────────────────────────

export interface TerminalItemData {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
  /** Display-only echo of the launching command for the tab agent icon
   *  (0.40.38). Survives restores where `command` is deliberately
   *  dropped (v2 daemon-owned) or the daemon row is gone (dead PTY /
   *  remote re-login). Never sent to spawn. */
  commandHint?: string
  sessionId?: string  // Kessel PTY id after v2 reconcile — not the chat key
  /** Provider conversation uuid (premint / --resume). Never the PTY id. */
  conversationId?: string
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
   *   - 'kessel'        — daemon-owned PTY (cli/sessions/v2/close).
   *   - 'alacritty-v2'  — pre-rename working name for 'kessel';
   *                       still dispatches to the Kessel pane.
   *   - 'alacritty'     (Legacy) — Tauri-local PTY (terminal_kill).
   */
  renderer?: 'alacritty' | 'alacritty-v2' | 'kessel'
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
  /** D9 — request intent: when true, TerminalPane asks the daemon to
   *  run this session in a sandbox (microVM backend) via the additive
   *  `sandbox` key on /cli/sessions/v2/spawn. Persisted in the
   *  serialized layout so a reattach re-asks. Nothing sets it today
   *  (default-OFF); dormant until the Linux microVM backend ships. */
  sandbox?: boolean
  /** D9 — resolved backend name stamped from the spawn response
   *  (`'microvm' | 'passthrough' | undefined`). THIS drives the
   *  orange tab marker (truthful — a degraded passthrough shows no
   *  orange). Runtime-only; intentionally NOT serialized — it is
   *  re-derived from the next spawn response on reattach. */
  sandboxBackend?: string
  /** API-origin session (`api-…` host-session / sandbox). Close minimizes. */
  fromApi?: boolean
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

/** Embedded Browser Tab (PRD prd-browser-pane-v1.md). The renderer keeps
 *  the URL, the page title, and a favicon `data:` / `blob:` URL. The page
 *  itself lives in a NATIVE child webview owned by src-tauri
 *  (browser_create / browser-<item id>), so nothing else here is canonical
 *  — no history, no scroll state. `url` is updated from `browser_current_url`
 *  polling so serialize captures where the user actually navigated, and
 *  restore re-creates the view there. `icon` absent on old layouts is the globe. */
export interface BrowserItemData {
  url: string
  title?: string
  /** Paintable favicon (`data:image/…` or `blob:`). Never an https URL. */
  icon?: string
  /** V21 / D3 — bumped only when THIS window asks the pane to navigate
   *  (`openUrlInPane`). BrowserPane navigates its live page on a change of
   *  this, never on a `url` change alone, so another window's navigation
   *  (arriving through the shared layout) does not move this window's page.
   *  Never serialized. */
  navSeq?: number
}

export interface Item {
  id: string
  type: 'terminal' | 'file-viewer' | 'agent' | 'browser'
  data: TerminalItemData | FileViewerItemData | AgentItemData | BrowserItemData
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
  /** D9 — sandbox REQUEST intent persisted across reattach. Only the
   *  intent is serialized; the resolved `sandboxBackend` is runtime-
   *  only and re-derived from the next spawn response. Optional, so
   *  older layouts missing it deserialize to undefined (default-OFF). */
  sandbox?: boolean
  /** Display-only hint of the launching command (0.40.38) — drives the
   *  tab agent icon across restores (incl. dead-PTY and remote
   *  re-login). NEVER restored into `command` itself: `command` in the
   *  spawn body would re-launch the agent on a cold restore. */
  commandHint?: string
  /** Provider conversation uuid (premint / --resume). Persisted so Chats
   *  can bind to the restored tab before argv is refilled. Never the
   *  Kessel PTY id; never restored into `command`. */
  conversationId?: string
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

/** Embedded Browser Tab item (browser-pane arc). ADDITIVE union member —
 *  no LAYOUT_SCHEMA_VERSION bump, following the established pattern for
 *  new optional shapes (SerializedAgentItem.sessionId, terminal
 *  `sandbox`): old layouts simply never contain a `'browser'` item, so
 *  they load unchanged, and readers that predate this type never receive
 *  one from their own saves. URL, display title, and favicon persist —
 *  a missing `icon` on an older save is the globe. The native child
 *  webview is re-created at that URL on restore. */
export interface SerializedBrowserItem {
  id: string
  type: 'browser'
  url?: string
  title?: string
  /** `data:image/…` or `blob:`. Omitted on layouts saved before favicons. */
  icon?: string
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
  | SerializedBrowserItem

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
  /** Tab-rename stickiness — persists a USER rename across relaunch so a
   *  locked tab stays locked (PTY/session titles still can't clobber it
   *  after restore). Mirrors `Tab.locked`. */
  locked?: boolean
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
  /** Tab-rename stickiness — a USER rename sets this true (daemon-canonical
   *  `tab_titles.locked`). Once locked, program-generated PTY/OSC/session
   *  titles (the auto-sync paths in `setTabTitle`) must NOT overwrite the
   *  user's chosen label. Hydrated from the daemon `tab-titles` snapshot /
   *  `tab_title_changed` broadcast and preserved across layout restore. */
  locked?: boolean
}

/** Options for addTab / addTabToGroup. `conversationId` is the provider
 *  chat uuid (Chats resume), stamped at create — never the Kessel PTY id. */
export interface AddTerminalTabOptions {
  title?: string
  command?: string
  args?: string[]
  locked?: boolean
  conversationId?: string
}

export interface TabsState {
  tabs: Tab[]
  activeTabId: string | null

  // Existing actions (signatures preserved)
  addTab: (cwd: string, options?: AddTerminalTabOptions) => string
  removeTab: (tabId: string, opts?: { forceReap?: boolean }) => void
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
  /** Spawn 409 `session_owned_elsewhere`: drop this pane from the open
   *  workspace and save. Does not POST v2/close. */
  releasePaneOwnedElsewhere: (paneId: string) => void
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
  /** Focus the chat this heartbeat is already running in, or attach
   *  the live PTY. Pinned chat is decided by the caller
   *  (`openHeartbeatTarget`) before this runs — pinned mode leaves
   *  `last_session_id` set, and a conversation search first would
   *  focus that leftover chat. Returns the tab id that was focused
   *  or attached, or null when there is nothing to open yet. */
  openHeartbeatTab: (
    projectPath: string,
    heartbeatName: string,
  ) => Promise<string | null>
  openFileAsTab: (filePath: string) => void
  openFileInPaneGroup: (tabId: string, paneGroupId: string, filePath: string) => void
  openDiffInPane: (tabId: string, filePath: string) => void
  pinPane: (tabId: string, paneGroupId: string) => void
  unpinPane: (tabId: string, paneGroupId: string) => void
  openFileInNewTab: (filePath: string) => void
  /** Browser-pane arc — open `url` in a NEW tab holding a single browser
   *  item (mirrors openFileInNewTab). This is the shared entry point for
   *  integrations (terminal URL clicks, daemon events, menus). Non-http(s)
   *  URLs still get a tab; the Rust side rejects them at browser_create
   *  and BrowserPane shows the error inline.
   *  `groupIndex` omitted or 0 appends to the primary strip. */
  openUrlInNewTab: (url: string, groupIndex?: number) => void
  /** Browser-pane arc — open `url` in the given tab's active pane group
   *  (mirrors openFileInPane): reuse the pane group's existing unpinned
   *  browser item when present (navigate-in-place), else append a new
   *  browser item. */
  openUrlInPane: (tabId: string, url: string) => void
  /** Browser-pane arc — stamp the polled current URL back onto a browser
   *  item so the next serialize captures where the user actually navigated
   *  (mirrors setFileViewerState). Does not write `tab.title`. Page title
   *  and favicon go through `applyBrowserPageMeta`. */
  setBrowserItemState: (tabId: string, paneGroupId: string, itemId: string, state: { url?: string; title?: string }) => void
  /** Page title + favicon from the child webview. Writes the item always.
   *  Writes `tab.title` only for an unlocked browser-only tab with a
   *  non-empty trimmed title. Does not set `locked` and does not call
   *  `setTabTitle` (that door drops harness-looking titles). */
  applyBrowserPageMeta: (
    tabId: string,
    paneGroupId: string,
    itemId: string,
    meta: { title?: string; icon?: string | null },
  ) => void
  /** `groupIndex` omitted or 0 appends to the primary strip. */
  openUntitledDocument: (cwd: string, groupIndex?: number) => void
  /** Set a tab's bar title.
   *  - USER renames (TabBar `commitRename`) pass `{ locked: true }` — that
   *    marks the tab locked locally AND in the daemon `tab_titles` store so
   *    the rename is sticky and program titles can't snap it back.
   *  - AUTO paths (PTY/OSC title listeners, daemon `label_changed`) call it
   *    WITHOUT `locked`; the action skips any tab already `locked` so the
   *    user's rename wins. */
  setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => void
  /** D9 — stamp the resolved sandbox backend name from the spawn
   *  response onto the matching terminal item. TerminalPane calls this
   *  after every successful spawn (fresh + reuse). `'microvm'` drives
   *  the orange tab marker; `'passthrough' | undefined` clears it. */
  setTerminalSandboxBackend: (terminalId: string, backend: string | undefined) => void
  setTerminalConversationId: (terminalId: string, conversationId: string | undefined) => void
  /** 0.39.39 (#676) — apply a daemon-canonical title to a tab WITHOUT
   *  re-POSTing (used by the `tab_title_changed` broadcast handler + the
   *  on-load `tab-titles` snapshot, so a rename in another client shows
   *  here). Also carries the daemon's `locked` flag so a remote/restored
   *  user-rename stays sticky here. No-op when the tab id isn't surfaced. */
  applyDaemonTabTitle: (tabId: string, title: string, locked?: boolean) => void
  renameTabByTitle: (oldTitle: string, newTitle: string) => void
  setTabDirty: (tabId: string, dirty: boolean) => void
  // ── Unsaved-changes leave guard support ──────────────────────────────
  /** Enumerate the dirty tab ids in the ACTIVE (in-view) workspace. `state.tabs`
   *  + `extraGroups` only ever hold the active workspace's tabs, so this is the
   *  set of unsaved tabs that would be lost by leaving. `projectId` is accepted
   *  for call-site clarity but the active view IS the leaving workspace. */
  dirtyTabIdsForProject: (projectId: string) => string[]
  /** Mark tabs so their FileViewerPane unmount-flush SKIPS the disk write
   *  (the user chose Discard). In-memory only — never persisted. */
  markTabsDiscardPending: (tabIds: string[]) => void
  /** Returns true (and clears the flag) if this tab was marked discard-pending.
   *  Called from the FileViewerPane unmount-flush right before it would write. */
  consumeDiscardPending: (tabId: string) => boolean
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
  addTabToGroup: (groupIndex: number, cwd: string, options?: AddTerminalTabOptions) => string
  removeTabFromGroup: (groupIndex: number, tabId: string, opts?: { forceReap?: boolean }) => void
  /** Kill every non-system tab in the strip, including API host-sessions
   *  (does not minimize). Drops the durable index so reboot cannot revive. */
  forceReapAllTabsInGroup: (groupIndex: number) => void
  setActiveTabInGroup: (groupIndex: number, tabId: string) => void
  moveTabToGroup: (fromGroup: number, toGroup: number, tabId: string) => void
  getGroupTabs: (groupIndex: number) => { tabs: Tab[], activeTabId: string | null }

  // Navigation history (back/forward) — tab strip, not the browser page.
  // ⌘[ / ⌘] in App.tsx call goBack / goForward. Page history is browser_*.
  navHistory: string[]     // stack of tabIds
  navIndex: number         // current position (-1 = no history)
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
  saveLayoutForWorkspace: (projectId: string, workspaceId: string, opts?: { force?: boolean }) => void
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
  /** 0.40.48: cancel the autosave debounce and save the active workspace
   *  layout NOW. For structural mutations that must hold across a remote
   *  round-trip (column split/unsplit). */
  flushLayoutPersist: (opts?: { allowEmpty?: boolean }) => void

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
          // D9 — persist the sandbox REQUEST intent only (so a
          // reattach re-asks). The resolved `sandboxBackend` stays
          // runtime-only and is re-derived from the spawn response.
          sandbox: d.sandbox,
          // Icon continuity across restores — live command wins, else
          // carry the prior hint forward. Never written back into
          // `command` on restore.
          commandHint: d.command ?? d.commandHint,
          ...(d.conversationId?.trim() ? { conversationId: d.conversationId.trim() } : {}),
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
      } else if (item.type === 'browser') {
        const d = item.data as BrowserItemData
        return {
          id: item.id,
          type: 'browser' as const,
          // `url` tracks browser_current_url polling (BrowserPane stamps
          // it via setBrowserItemState), so restore lands where the user
          // actually navigated — not the URL the pane was opened with.
          url: d.url,
          title: d.title,
          icon: d.icon,
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
    // T15 — emit locked: false when unlocked so overlay can tell omit vs false.
    locked: tab.locked === true,
  }
}

/** v2 restore: paneGroupId is SSOT. Daemon owns command/args/sessionId.
 *  `commandHint` is display-only — never copied into `command`. */
function restoredV2TerminalData(
  t: SerializedTerminalItemV2,
  paneGroupId: string,
  cwd: string,
): TerminalItemData {
  return {
    terminalId: paneGroupId,
    cwd,
    renderer: currentRenderer(),
    heartbeatName: t.heartbeatName,
    projectPath: t.projectPath,
    surfacedAgentName: t.surfacedAgentName,
    attachAgentName: t.attachAgentName,
    fromApi:
      typeof t.attachAgentName === 'string' &&
      t.attachAgentName.startsWith('api-'),
    sandbox: t.sandbox,
    commandHint: t.commandHint,
    ...(t.conversationId?.trim() ? { conversationId: t.conversationId.trim() } : {}),
  }
}

/** T15 — never wipe a known conversationId to null when rebuilding a tab. */
function preserveConversationIds(built: Tab, live: Tab | undefined): Tab {
  if (!live) return built
  let changed = false
  const newPaneGroups = new Map<string, PaneGroup>()
  for (const [pgId, pg] of built.paneGroups) {
    const livePg = live.paneGroups.get(pgId)
    if (!livePg) {
      newPaneGroups.set(pgId, pg)
      continue
    }
    const liveCid = (() => {
      for (const item of livePg.items) {
        if (item.type !== 'terminal') continue
        const id = conversationIdFromTerminal(item.data as TerminalItemData)?.trim()
        if (id) return id
      }
      return undefined
    })()
    if (!liveCid) {
      newPaneGroups.set(pgId, pg)
      continue
    }
    let pgChanged = false
    const newItems = pg.items.map((item) => {
      if (item.type !== 'terminal') return item
      const d = item.data as TerminalItemData
      if (d.conversationId?.trim()) return item
      pgChanged = true
      changed = true
      return { ...item, data: { ...d, conversationId: liveCid } }
    })
    newPaneGroups.set(pgId, pgChanged ? { ...pg, items: newItems } : pg)
  }
  return changed ? { ...built, paneGroups: newPaneGroups } : built
}

/** Serialize a WorkspaceTabSnapshot (background workspace with live tabs).
 *  per-client-view-state.md (Phase 1) — like `serializeCurrentLayout`, the
 *  canonical background-workspace layout carries STRUCTURE only; the snapshot's
 *  `activeTabId` (top-level + per-split-group) is per-client VIEW state and is
 *  intentionally omitted so a background-workspace save can't leak this
 *  client's selection to peers. */
function serializeSnapshot(snapshot: WorkspaceTabSnapshot): SerializedLayout {
  return serializeColumnsLayout(snapshot.tabs, snapshot.extraGroups, snapshot.splitCount)
}

/** The shared layout for a set of columns (D1: columns and their tabs are
 *  shared). Per-window view state is never written (D2): not the focused
 *  column (`activeGroupIndex`), not any column's selected tab. When column 0
 *  is empty but a later column is not, the columns move left — every reader
 *  treats an empty `tabs` as "no layout". */
function serializeColumnsLayout(
  tabs: Tab[],
  extraGroups: Array<{ tabs: Tab[] }>,
  splitCount: number,
): SerializedLayout {
  const raw: Tab[][] = [tabs, ...extraGroups.map((g) => g.tabs)]
  while (raw.length < splitCount) raw.push([])
  const cols = collapseEmptyLeadingColumns(raw)
  const [first, ...rest] = cols
  return {
    version: LAYOUT_SCHEMA_VERSION,
    tabs: (first ?? []).map(serializeTab),
    extraGroups: rest.length > 0 ? rest.map((c) => ({ tabs: c.map(serializeTab) })) : undefined,
    splitCount: cols.length > 1 ? cols.length : undefined,
  }
}
const LAYOUT_SAVE_RETRY_BASE_MS = 3000
const LAYOUT_SAVE_MAX_BLIND_RETRIES = 3

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

/** Fields of a terminal item that the shared layout carries. Everything else
 *  on a live item (daemon command/args/sessionId, resolved sandbox, spawn
 *  time) is runtime and survives a remote restore of the same item. */
function carryLiveTerminalData(restored: TerminalItemData, live: TerminalItemData): TerminalItemData {
  return {
    ...live,
    heartbeatName: restored.heartbeatName,
    projectPath: restored.projectPath,
    surfacedAgentName: restored.surfacedAgentName,
    attachAgentName: restored.attachAgentName,
    fromApi: restored.fromApi,
    sandbox: restored.sandbox,
    commandHint: restored.commandHint ?? live.commandHint,
    conversationId: restored.conversationId ?? live.conversationId,
  }
}

/** Restore one serialized item. Reuses the saved id (V20) when it is a string
 *  not already used in this restore; mints one otherwise (old layouts,
 *  duplicates). */
function restoreSerializedItem(
  si: SerializedItem,
  paneGroupId: string,
  cwd: string,
  liveItemsById: Map<string, Item>,
  usedItemIds: Set<string>,
): Item {
  const savedId =
    typeof si.id === 'string' && si.id.length > 0 && !usedItemIds.has(si.id) ? si.id : null
  const id = savedId ?? crypto.randomUUID()
  usedItemIds.add(id)
  const live = savedId ? liveItemsById.get(savedId) : undefined
  if (si.type === 'terminal') {
    // After migrateLayoutToV2, terminal items are v2 shape: only
    // paneGroupId + heartbeat metadata. cwd/command/args/sessionId/renderer
    // come from the daemon at reconcile time — or, for a pane this window
    // already shows, from the live item.
    const restored = restoredV2TerminalData(si as SerializedTerminalItemV2, paneGroupId, cwd)
    const liveData =
      live?.type === 'terminal' && (live.data as TerminalItemData).terminalId === paneGroupId
        ? (live.data as TerminalItemData)
        : undefined
    return {
      id,
      type: 'terminal' as const,
      data: liveData ? carryLiveTerminalData(restored, liveData) : restored,
    }
  }
  if (si.type === 'agent') {
    const restoredSection = si.section ?? 'inbox'
    return {
      id,
      type: 'agent' as const,
      data: {
        agentName: si.agentName ?? '',
        projectPath: si.projectPath ?? cwd,
        section: restoredSection,
        // 0.37.12 — restore the pinned chat tab's Claude session id (when
        // present). AgentChatPane reads this as a hint and resumes the same
        // session without a daemon round-trip. 0.37.12 P1C scrub: only the
        // chat tab carries it; a leaked value on inbox tabs is dropped. Split
        // columns restore it the same way as column 0.
        sessionId: restoredSection === 'chat' ? si.sessionId : undefined,
      },
    }
  }
  if (si.type === 'browser') {
    // URL, title, and favicon persist; the native child webview is created
    // at that URL by BrowserPane on first visibility. A missing icon (older
    // layouts) is the globe. An empty url = address bar only. `navSeq` (this
    // window's own navigate requests, V21) is never saved and survives.
    const liveNav =
      live?.type === 'browser' ? (live.data as BrowserItemData).navSeq : undefined
    return {
      id,
      type: 'browser' as const,
      data: {
        url: si.url ?? '',
        title: si.title,
        icon: si.icon,
        ...(liveNav !== undefined ? { navSeq: liveNav } : {}),
      },
    }
  }
  const liveMode =
    live?.type === 'file-viewer' ? (live.data as FileViewerItemData).mode : undefined
  return {
    id,
    type: 'file-viewer' as const,
    data: {
      filePath: si.filePath ?? '',
      scrollTop: si.scrollTop,
      cursorPos: si.cursorPos,
      ...(liveMode !== undefined ? { mode: liveMode } : {}),
    },
    pinned: si.pinned ?? false,
  }
}

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
  options?: { command?: string; args?: string[]; conversationId?: string }
): PaneGroup {
  const itemId = crypto.randomUUID()
  const conversationId = options?.conversationId?.trim()
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
          ...(conversationId ? { conversationId } : {}),
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

/** Create a PaneGroup with a single browser item (browser-pane arc). */
function makeBrowserPaneGroup(paneGroupId: string, url: string): PaneGroup {
  return {
    id: paneGroupId,
    items: [
      {
        id: crypto.randomUUID(),
        type: 'browser',
        data: { url },
      },
    ],
    activeItemIndex: 0,
  }
}

/** Best-effort display label for a browser tab: hostname, else the raw
 *  string the user typed. */
function browserTabTitle(url: string): string {
  try {
    return new URL(url).hostname || url
  } catch {
    return url || 'Browser'
  }
}

/** True when every pane item is a browser. A mixed tab keeps its own strip title. */
function tabIsBrowserOnly(tab: Tab): boolean {
  let saw = false
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'browser') return false
      saw = true
    }
  }
  return saw
}

function browserDataWithoutIcon(data: BrowserItemData): BrowserItemData {
  const next: BrowserItemData = { url: data.url }
  if (data.title !== undefined) next.title = data.title
  return next
}

/** Item title/icon always. Strip title only when unlocked, browser-only, and non-empty. */
function withBrowserPageMeta(
  tab: Tab,
  paneGroupId: string,
  itemId: string,
  meta: { title?: string; icon?: string | null },
): Tab {
  const hasTitle = typeof meta.title === 'string'
  const hasIcon = Object.prototype.hasOwnProperty.call(meta, 'icon')
  if (!hasTitle && !hasIcon) return tab
  const pg = tab.paneGroups.get(paneGroupId)
  if (!pg) return tab
  const trimmed = hasTitle ? (meta.title as string).trim() : ''
  const nextIcon = hasIcon ? paintableBrowserIcon(meta.icon) : null
  let itemsChanged = false
  const items = pg.items.map((item) => {
    if (item.id !== itemId || item.type !== 'browser') return item
    const prev = item.data as BrowserItemData
    let data: BrowserItemData = prev
    if (hasTitle && prev.title !== trimmed) {
      data = { ...data, title: trimmed }
    }
    if (hasIcon) {
      if (nextIcon) {
        if (data.icon !== nextIcon) data = { ...data, icon: nextIcon }
      } else if (data.icon) {
        data = browserDataWithoutIcon(data)
      }
    }
    if (data === prev) return item
    itemsChanged = true
    return { ...item, data }
  })
  const paneGroups = itemsChanged ? new Map(tab.paneGroups) : tab.paneGroups
  if (itemsChanged) paneGroups.set(paneGroupId, { ...pg, items })
  let title = tab.title
  if (hasTitle && trimmed && tab.locked !== true && tabIsBrowserOnly(tab)) {
    title = trimmed
  }
  if (!itemsChanged && title === tab.title) return tab
  return { ...tab, title, paneGroups }
}

/** Append `tab` to column `groupIndex`. 0 is the primary strip; a missing extra column is a no-op. */
function placedTab(
  state: {
    tabs: Tab[]
    extraGroups: Array<{ tabs: Tab[]; activeTabId: string | null }>
  },
  groupIndex: number,
  tab: Tab,
): { tabs: Tab[]; activeTabId: string } | { extraGroups: Array<{ tabs: Tab[]; activeTabId: string | null }> } {
  if (groupIndex <= 0) {
    return { tabs: [...state.tabs, tab], activeTabId: tab.id }
  }
  const newGroups = [...state.extraGroups]
  const gi = groupIndex - 1
  if (gi >= 0 && gi < newGroups.length) {
    newGroups[gi] = {
      tabs: [...newGroups[gi].tabs, tab],
      activeTabId: tab.id,
    }
  }
  return { extraGroups: newGroups }
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
  kind?: string
  handle?: string
  conversationId?: string
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

/** Strip one pane id out of a mosaic split. One-terminal tabs are not
 *  touched here — `dropSurfacedTabsForSessionRemoval` owns those. */
function stripSplitPane(tabs: Tab[], paneId: string): { tabs: Tab[]; changed: boolean } {
  let changed = false
  const next = tabs.map((tab) => {
    if (tab.paneGroups.size <= 1 || !tab.paneGroups.has(paneId)) return tab
    changed = true
    const paneGroups = new Map(tab.paneGroups)
    paneGroups.delete(paneId)
    return {
      ...tab,
      paneGroups,
      mosaicTree: removePaneFromTree(tab.mosaicTree, paneId),
    }
  })
  return { tabs: next, changed }
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

// ── P3c (D2) — generic API-spawned tab adoption + reaper close ────────────
//
// The workspace-scoped `subscribeForActiveWorkspace.onAdded` consumer above
// only adopts `tab-<paneGroupId>` sessions whose cwd lives under the ACTIVE
// workspace path. An API-spawned cell (POST /v1/sandboxes OR host-sessions)
// registers under a host-minted `api-<principal>-<uuid>` agent_name. Sandbox
// cells use an EPHEMERAL cwd that matches no registered workspace; host
// sessions sit in a real workspace path but still use the `api-` namespace
// (so the `tab-` gate above ignores them). The daemon forwards absolute-cwd
// `SessionAdded`/`SessionRemoved` to the APP-LEVEL subscriber, which
// dispatches to `onSessionAddedApp` / `onSessionRemovedApp`.
//
// Scope gate (add): act ONLY on events carrying a `sandbox_backend` label.
// The daemon stamps that for real sandboxes (`microvm`) AND for the host-
// sessions family (`host` — see v2_session_map's api- namespace OR). Bare
// `SessionAdded` with no backend stays default-OFF (workspace consumer owns
// `tab-` adoption).
//
// Scope gate (remove): act ONLY on `api-…` agent names. When the idle
// sandbox-reaper (or any other path) kills the PTY, ChildExit unregisters
// the v2 session and emits SessionRemoved. Closing the audit tab here is
// what makes post-reap resume open a *new* tab: resume reuses the caller's
// stored session id, and the de-dupe below keys on sessionId — a leftover
// zombie tab would swallow the new SessionAdded.

/** True when a terminal item attached to (or spawned as) `agentName`, or
 *  carrying `sessionId`, is ALREADY surfaced in any tab across all groups.
 *  The de-dupe guard for API-spawned adoption: the window that already adopted
 *  this cell (or a re-delivered event) must NOT create a second tab. Mirrors
 *  the AgentChatPane remount-guard's identity check (session_id) + the
 *  `isPaneGroupSurfaced` "already represented" check, matched on the attach
 *  agent_name since the API cell has no `tab-`-shaped paneGroupId. */
function tabMatchesApiSession(tab: Tab, agentName: string, sessionId: string): boolean {
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const d = item.data as TerminalItemData
      if (d.attachAgentName === agentName) return true
      if (sessionId && d.sessionId === sessionId) return true
    }
  }
  return false
}

function isApiSandboxSessionSurfaced(
  state: {
    tabs: Tab[]
    extraGroups: Array<{ tabs: Tab[]; activeTabId: string | null }>
    backgroundWorkspaces?: Record<string, WorkspaceTabSnapshot>
  },
  agentName: string,
  sessionId: string,
): boolean {
  for (const tab of state.tabs) {
    if (tabMatchesApiSession(tab, agentName, sessionId)) return true
  }
  for (const group of state.extraGroups) {
    for (const tab of group.tabs) {
      if (tabMatchesApiSession(tab, agentName, sessionId)) return true
    }
  }
  // Also de-dupe against parked (non-active workspace) strips — host-session
  // adopt may place cells there so the focused workspace is not polluted.
  if (state.backgroundWorkspaces) {
    for (const snap of Object.values(state.backgroundWorkspaces)) {
      for (const tab of snap.tabs) {
        if (tabMatchesApiSession(tab, agentName, sessionId)) return true
      }
      for (const group of snap.extraGroups) {
        for (const tab of group.tabs) {
          if (tabMatchesApiSession(tab, agentName, sessionId)) return true
        }
      }
    }
  }
  return false
}

/** True when removing the tab is safe under the API-session reaper path.
 *  Skip pinned/system tabs and any tab that isn't a pure attach to this
 *  `api-…` agent (splits / agent panes / file viewers keep their own
 *  lifecycle — same invariant as `tabIsDropCandidateForSessionRemoval`). */
function tabIsDropCandidateForApiSessionRemoval(tab: Tab, agentName: string): boolean {
  if (tab.isSystemAgent) return false
  if (tab.paneGroups.size !== 1) return false
  const pg = [...tab.paneGroups.values()][0]
  if (!pg) return false
  if (pg.items.length === 0) return false
  for (const item of pg.items) {
    if (item.type !== 'terminal') return false
    const d = item.data as TerminalItemData
    if (d.attachAgentName !== agentName) return false
  }
  return true
}

/** P3c (D2) + host-sessions — adopt an API-spawned session into a new cockpit
 *  tab. Returns true when a tab was adopted, false when ignored (no backend
 *  label, or already surfaced — the de-dupe). Exported for unit testing.
 *
 *  The tab carries `attachAgentName = event.agent_name` so when its
 *  `TerminalPane` mounts and issues the idempotent v2/spawn, find-or-spawn
 *  returns the EXISTING daemon session (reused:true) — it ATTACHES via the
 *  grid WS, it does NOT mint a duplicate PTY. Real sandboxes set
 *  `sandbox: true` (daemon re-echoes the backend); host sessions
 *  (`sandbox_backend: "host"`) attach without requesting a jail.
 *  `sandboxBackend` is stamped from the event so TabBar can light the D9
 *  orange marker for microvm cells. The tab is appended WITHOUT switching
 *  the active tab — surfacing an externally-spawned cell must never yank
 *  the user off their current tab. */
function isApiOriginTerminal(data: TerminalItemData): boolean {
  if (data.fromApi) return true
  const agent = data.attachAgentName
  return typeof agent === 'string' && agent.startsWith('api-')
}

// ── 0.39.39 (#676/#677) tab-title snapshot + remote-reorder re-fetch ───────

interface DaemonTabTitle {
  projectId: string
  tabId: string
  title: string
  /** Tab-rename stickiness — true when the title is a USER rename the
   *  daemon marked locked. Hydrated onto `Tab.locked` so the auto-sync
   *  skip in `setTabTitle` keeps program titles off the locked tab.
   *  Optional for daemons pre-dating the `tab_titles.locked` column. */
  locked?: boolean
}

/** Stable identity for matching a tab across a layout save/restore round-trip.
 *  `paneGroupId`s are preserved on the main group (restoreLayout "Reuse the
 *  saved ID", tabs.ts:2839), but the tab's own `id` is re-minted on restore.
 *  So a tab's signature is its sorted, joined paneGroupId set — this matches a
 *  serialized tab to the live `Tab` that owns the same panes regardless of the
 *  re-minted tab id.
 *
 *  The joiner is written as the ESCAPE sequence backslash-u0000, never a raw NUL byte: a
 *  raw NUL makes grep/rg classify this whole FILE as binary and silently skip
 *  it, which turned the 2026-07-02 PTY-leak forensics into a false "removeTab
 *  no longer calls /cli/sessions/v2/close" finding (the call was here all
 *  along — the audit tools just couldn't see into the file). Same runtime
 *  string, text-searchable source. */
function serializedTabSignature(t: SerializedTab): string {
  const pgIds = t.paneGroups ? Object.keys(t.paneGroups) : []
  return pgIds.slice().sort().join('\u0000')
}

function liveTabSignature(t: Tab): string {
  return Array.from(t.paneGroups.keys()).slice().sort().join('\u0000')
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

// ── The store factory ────────────────────────────────────────────────────

/** A `workspace:*` op from the Workspace Assistant, as the window-level
 *  router (`lib/workspace-ops-router.ts`) hands it to the focused room. */
export type WorkspaceOp =
  | { kind: 'split-pane'; payload: WsSplitPanePayload }
  | { kind: 'close-pane'; payload: WsClosePanePayload }
  | { kind: 'open-document'; payload: WsOpenDocumentPayload }
  | { kind: 'open-terminal'; payload: WsOpenTerminalPayload }
  | { kind: 'new-tab'; payload: WsNewTabPayload }
  | { kind: 'close-tab'; payload: WsCloseTabPayload }
  | { kind: 'arrange'; payload: LayoutDescriptor }

/** The room-level operations of one tabs store instance that are not store
 *  actions (they used to be module functions bound to THE store). */
export interface TabsRoomApi {
  /** The server this room's requests, sockets and saves go to. */
  readonly scope: ServerScope
  readonly kind: 'primary' | 'pinned'
  /** The one workspace a pinned room holds; null for the primary room. */
  readonly workspace: TabsRoomWorkspace | null
  /** MS67 — may this room call this computer's Tauri commands? */
  readonly localCommands: boolean
  /** Ensure the pinned Chat/Inbox tabs for the room's active workspace. */
  ensurePinnedAgentTabForMode(agentMode: string, projectPath: string): void
  /** Spawn 409 `session_owned_elsewhere`: drop the pane, no close. */
  releasePaneOwnedElsewhere(paneId: string): void
  dropTabAfterFailedSidecarRefresh(paneGroupId: string): void
  placeClickedSandboxTab(args: { groupIndex: number; cwd: string; sessionId: string; agentName: string }): void
  adoptApiSandboxSession(event: SessionAddedEvent): boolean
  openApiHostSessionTab(event: SessionAddedEvent): boolean
  minimizeApiSessionsForWorkspace(workspacePath: string): number
  dropApiSpawnedSession(event: SessionRemovedEvent): boolean
  hydrateApiSandboxSessions(): Promise<number>
  /** Wire the room's server's app-bus API-session adoption. */
  initApiSandboxTabAdoption(): UnsubscribeFn
  /** Wire the room's server's app-bus `open_url` → browser tab. */
  initOpenUrlBrowserTabs(): UnsubscribeFn
  /** Apply one `workspace:*` op to this room's tabs. */
  applyWorkspaceOp(op: WorkspaceOp): void
  /** Primary only: the #625 server-switch reset. */
  resetForHostSwitch(): void
  /** Pinned only: load the room's one workspace. */
  open(): Promise<void>
  /** Pinned only: flush the based save, close sockets and timers. */
  dispose(): Promise<void>
  /** Test hooks into the instance's layout-sync bookkeeping. */
  readonly __test: {
    layoutRevisions: Map<string, number>
    ackedLayouts: Map<string, { canonical: string; layout: SerializedLayout }>
    setAckedLayout(key: string, layout: SerializedLayout): void
    isLayoutSaveSuppressed(): boolean
    hasLoadedWorkspaceSessions(): boolean
    activeSessionEventsKey(): string | null
    tryReorderTabsInPlace(key: string, layout: SerializedLayout): boolean
    resetLayoutSync(): void
  }
}

/** One room's tabs store: a zustand hook-and-API plus its room operations. */
export type TabsStore = UseBoundStore<StoreApi<TabsState>> & { readonly room: TabsRoomApi }

/** Build one room's tabs store. See "One tabs store per room" above. */
export function createTabsStore(binding: TabsRoomBinding): TabsStore {
  const { scope, deps } = binding
  const isPrimary = binding.workspace === null
  /** Home M4: a view-only (preview) pinned room — no layout save, no close,
   *  no tab-title write on its server. Its scope's request layer refuses
   *  every other write too (`viewOnlyScope`). */
  const readOnly = scope.viewOnly === true

  /** A primary-room-only path reached from a pinned room is a bug: fail
   *  loudly instead of stashing, bulk-loading or switching servers. */
  function assertPrimary(what: string): void {
    if (!isPrimary) {
      throw new Error(`[tabs] ${what} is primary-room only (pinned room on ${scope.hostKey})`)
    }
  }
  function assertPinned(what: string): void {
    if (isPrimary) throw new Error(`[tabs] ${what} is pinned-room only`)
  }
  /** A pinned room holds exactly one workspace. */
  function assertRoomWorkspace(projectId: string, workspaceId: string): void {
    const ws = binding.workspace
    if (ws && (ws.projectId !== projectId || ws.workspaceId !== workspaceId)) {
      throw new Error(
        `[tabs] pinned room ${scope.hostKey}|${ws.projectId}:${ws.workspaceId} cannot load ${projectId}:${workspaceId}`,
      )
    }
  }
  function projectDefaultAgentFor(projectId: string | null): string | undefined {
    if (!projectId) return undefined
    return deps.projectDefaultAgent(projectId)
  }


  /** Phase 2.5 fix (finding #547) — gate for `loadWorkspaceSessionsFromDb`.
   *  Flips to true on the first successful load (regardless of whether the
   *  user has any saved layouts — empty list is still a successful load).
   *  Used only to suppress the reconnect-driven retry once the baseline
   *  is established; tabs.ts doesn't gate writes the way panels/timer/
   *  focus-groups/projects do because workspace layout writes are user-
   *  driven (tab open/close/move) rather than side-effects of UI mount. */
  let hasLoadedWorkspaceSessions = false

  // Tabs whose pending FileViewerPane unmount-flush must be SKIPPED (the user
  // chose Discard). In-memory module state — deliberately NOT in zustand store
  // state so it is never serialized into a saved layout. Drained by the pane's
  // unmount cleanup via `consumeDiscardPending`.
  const _discardPendingTabs = new Set<string>()

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

  /** Record a layout `revision` seen on a `TabOrderChanged` at or below our
   *  base. Monotonic — never moves backwards. Tolerates the old daemon's
   *  `{success}`-only response (no revision). */
  function recordLayoutRevision(key: string, revision: unknown): void {
    if (typeof revision !== 'number') return
    const prev = layoutRevisions.get(key) ?? 0
    if (revision > prev) layoutRevisions.set(key, revision)
  }

  /** Set the base revision to exactly what the daemon just told us for the
   *  layout this window now holds (a load, an adopted refetch, or our own save
   *  landing). `undefined` (an older daemon) forgets the base, so the next save
   *  goes without `baseRevision` and stays last-write-wins (D4). */
  function setLayoutRevisionExact(key: string, revision: unknown): void {
    if (typeof revision === 'number') layoutRevisions.set(key, revision)
    else layoutRevisions.delete(key)
  }

  /** Last layout this window knows the daemon holds, per workspace key. */
  const ackedLayouts = new Map<string, AckedLayout>()

  function setAckedLayout(key: string, layout: SerializedLayout): void {
    ackedLayouts.set(key, { canonical: canonicalLayoutJson(layout), layout })
  }

  /** Revision of the cached `workspaceLayouts[key]` entry, when known. The
   *  cached restore path takes its base from here, never from a newer save. */
  const cacheRevisions = new Map<string, number>()

  /** Workspace cwd per key, so a conflict merge can restore the merged layout. */
  const layoutCwds = new Map<string, string>()

  /** Keys with a remote refetch waiting or running. `session_added` adoption
   *  waits for it (V22) so a peer's new split terminal is not pulled into
   *  this window's column 0. */
  const refetchInFlight = new Set<string>()
  const deferredAdoptions = new Map<string, Array<() => void>>()

  function deferAdoption(key: string, fn: () => void): void {
    let list = deferredAdoptions.get(key)
    if (!list) {
      list = []
      deferredAdoptions.set(key, list)
    }
    list.push(fn)
  }

  function drainDeferredAdoptions(key: string): void {
    const list = deferredAdoptions.get(key)
    if (!list) return
    deferredAdoptions.delete(key)
    for (const fn of list) fn()
  }

  const layoutLanes = new Map<string, LayoutLane>()

  function laneFor(key: string): LayoutLane {
    let lane = layoutLanes.get(key)
    if (!lane) {
      lane = { busy: false, ops: [], queuedSave: null, idle: [] }
      layoutLanes.set(key, lane)
    }
    return lane
  }

  /** Run `op` exclusively for `key`. Starts synchronously when the lane is
   *  idle (so a save's POST goes out in the same tick, as before). */
  function runInLayoutLane(key: string, op: () => Promise<void>): Promise<void> {
    const lane = laneFor(key)
    if (lane.busy) {
      return new Promise<void>((resolve) => {
        lane.ops.push(() => op().finally(resolve))
      })
    }
    lane.busy = true
    return op()
      .catch(logLaneError)
      .finally(() => releaseLayoutLane(key))
  }

  function releaseLayoutLane(key: string): void {
    const lane = laneFor(key)
    const next = lane.ops.shift()
    if (next) {
      void next()
        .catch(logLaneError)
        .finally(() => releaseLayoutLane(key))
      return
    }
    const job = lane.queuedSave
    if (job) {
      lane.queuedSave = null
      void runLayoutSaveJob(key, job)
        .catch(logLaneError)
        .finally(() => releaseLayoutLane(key))
      return
    }
    lane.busy = false
    const waiters = lane.idle.splice(0)
    for (const w of waiters) w()
  }

  /** Resolve once nothing is running or queued for `key` — including saves
   *  queued while waiting (each carries a base-advancing revision the caller
   *  must see before judging a broadcast). Resolves at once when idle. */
  function waitLayoutIdle(key: string): Promise<void> {
    const lane = laneFor(key)
    if (!lane.busy) return Promise.resolve()
    return new Promise<void>((resolve) => lane.idle.push(resolve))
  }

  /** Queue or send a layout save for `key`. Latest wins while a save or
   *  refetch is in flight. Resolves when this save (or the one that replaced
   *  it) has settled. */
  function submitLayoutSave(key: string, job: LayoutSaveJob): Promise<void> {
    // Home M4: a view-only room shows its server's tabs and never saves
    // them (no layout write reaches B until M5).
    if (readOnly) return Promise.resolve()
    const lane = laneFor(key)
    if (lane.busy) {
      lane.queuedSave = job
      return waitLayoutIdle(key)
    }
    lane.busy = true
    return runLayoutSaveJob(key, job)
      .catch(logLaneError)
      .finally(() => releaseLayoutLane(key))
  }

  /** The body of one save. Runs inside the lane. */
  async function runLayoutSaveJob(key: string, job: LayoutSaveJob, attempt = 0): Promise<void> {
    const layout = job.produce()
    if (!layout) return
    const canonical = canonicalLayoutJson(layout)
    if (!job.force && ackedLayouts.get(key)?.canonical === canonical) return
    const base = layoutRevisions.get(key)
    let res: { success?: boolean; revision?: number } | undefined
    try {
      res = await daemonCliPost<{ success?: boolean; revision?: number }>(scope, 'workspace-layouts/save', {
        projectId: job.projectId,
        workspaceId: job.workspaceId,
        layoutJson: JSON.stringify(layout),
        ...(base !== undefined ? { baseRevision: base } : {}),
      })
    } catch (err) {
      if (isLayoutRevisionConflict(err)) {
        if (job.conflict === 'merge' && attempt < LAYOUT_CONFLICT_MAX_ATTEMPTS) {
          await mergeAfterLayoutConflict(key, job, attempt + 1)
          return
        }
        console.warn(`[tabs] ${job.label}: layout changed elsewhere; dropped this write for ${key}`)
        // The cached copy is behind the daemon — let the next open re-read it.
        if (job.conflict === 'drop') forgetCachedLayout(key)
        return
      }
      console.error(`[tabs] ${job.label} failed:`, err)
      if (job.retry) rearmLayoutSave(err)
      return
    }
    layoutSaveSucceeded()
    if (typeof res?.revision === 'number') {
      layoutRevisions.set(key, res.revision)
      cacheRevisions.set(key, res.revision)
    }
    setAckedLayout(key, layout)
  }

  function forgetCachedLayout(key: string): void {
    cacheRevisions.delete(key)
    const cur = store.getState().workspaceLayouts
    if (!(key in cur)) return
    const { [key]: _drop, ...rest } = cur
    store.setState({ workspaceLayouts: rest })
  }

  /** GET the stored layout with its revision (V14). An older daemon ignores
   *  `with_revision` and answers with the bare JSON string. */
  async function fetchLayoutWithRevision(projectId: string, workspaceId: string): Promise<FetchedLayout> {
    const res = await daemonCliGet<unknown>(scope, 'workspace-layouts/load', {
      project_id: projectId,
      workspace_id: workspaceId,
      with_revision: '1',
    })
    let json: string | null = null
    let revision: number | undefined
    if (typeof res === 'string') {
      json = res
    } else if (res && typeof res === 'object' && 'layoutJson' in (res as Record<string, unknown>)) {
      const obj = res as { layoutJson?: unknown; revision?: unknown }
      json = typeof obj.layoutJson === 'string' ? obj.layoutJson : null
      revision = typeof obj.revision === 'number' ? obj.revision : undefined
    }
    if (!json) return { layout: null, revision }
    return { layout: JSON.parse(json) as SerializedLayout, revision }
  }

  /** True when this window holds a layout change the daemon has not confirmed:
   *  the autosave debounce is armed, or the current layout differs from acked. */
  function hasUnackedLocalLayout(key: string): boolean {
    // No acked layout (an older daemon, or nothing loaded yet): there is no
    // base to merge against, so the remote layout wins as before.
    const acked = ackedLayouts.get(key)
    if (!acked) return false
    if (persistDebounceTimer !== null) return true
    const st = store.getState()
    if (st.activeWorkspaceKey !== key) return false
    return canonicalLayoutJson(st.serializeCurrentLayout()) !== acked.canonical
  }

  /** Apply a layout from the daemon to this window without echoing a save.
   *  A pure reorder permutes the live tabs; anything else restores with the
   *  saved item ids, so a surviving pane never remounts (V3). */
  function applyLayoutSilently(key: string, layout: SerializedLayout, cwd: string): void {
    withLayoutSaveSuppressed(() => {
      if (tryReorderTabsInPlace(key, layout)) return
      store.setState((s) => ({ workspaceLayouts: { ...s.workspaceLayouts, [key]: layout } }))
      store.getState().restoreLayout(layout, cwd)
    })
  }

  function cwdForLayoutKey(key: string): string {
    const known = layoutCwds.get(key)
    if (known) return known
    const st = store.getState()
    for (const t of [...st.tabs, ...st.extraGroups.flatMap((g) => g.tabs)]) {
      for (const pg of t.paneGroups.values()) {
        for (const item of pg.items) {
          if (item.type === 'terminal') return (item.data as TerminalItemData).cwd
        }
      }
    }
    return ''
  }

  /** 409 on an open-workspace save (V18): fetch the newer layout, merge this
   *  window's change onto it by tab id, adopt the result, and save it with the
   *  fresh base. Runs inside the lane. */
  async function mergeAfterLayoutConflict(key: string, job: LayoutSaveJob, attempt: number): Promise<void> {
    const fetched = await fetchLayoutWithRevision(job.projectId, job.workspaceId)
    if (store.getState().activeWorkspaceKey !== key) return
    const local = job.produce()
    if (!local) return
    const remote = fetched.layout ?? EMPTY_SERIALIZED_LAYOUT
    const base = ackedLayouts.get(key)?.layout ?? null
    const merged = mergeSerializedLayouts(base, local, remote)
    setLayoutRevisionExact(key, fetched.revision)
    if (fetched.revision !== undefined) cacheRevisions.set(key, fetched.revision)
    setAckedLayout(key, remote)
    if (persistDebounceTimer) {
      clearTimeout(persistDebounceTimer)
      persistDebounceTimer = null
    }
    console.warn(
      `[tabs] layout conflict for ${key}: merged this window's change onto revision ${fetched.revision ?? '?'}`,
    )
    applyLayoutSilently(key, merged, cwdForLayoutKey(key))
    await runLayoutSaveJob(key, job, attempt)
  }

  // ── Terminal close helpers ───────────────────────────────────────────────

  /**
   * Close a terminal session on the appropriate backend for its
   * renderer. This is the A6 contract (.k2so/prds/alacritty-v2.md):
   * a DELIBERATE tab close closes the daemon session. Kessel requires
   * this explicit call because its component unmount cleanup
   * intentionally does NOT close the daemon session — that's what
   * makes workspace swap + Tauri restart retain sessions.
   *
   * Close-semantics table (2026-07-02 PTY-leak incident audit —
   * pinned by tabs-close-contract.test.ts):
   *
   *   CLOSES the daemon session (routes here):
   *     - removeTab            (X-click / Cmd+W / context-menu Close /
   *                             Close Others / Close All — all via
   *                             removeTabFromGroup, incl. split columns)
   *     - removePaneFromTab    (pane close inside a tab)
   *     - closeItemInPaneGroup (item close inside a pane group)
   *
   *   DOES NOT close (by design — never routes here):
   *     - workspace switch / stash (`clearAllTabs` is view-clear only;
   *       see its 0.38.0 commit-5 comment)
   *     - app quit / TerminalPane unmount (detached-session design)
   *     - pinned system agent tabs (`removeTab` early-returns on
   *       `isSystemAgent`; the canonical workspace chat session persists)
   *     - remote `session_removed` push (daemon already closed it; the
   *       handler only drops the local view)
   *     - retained-chat eviction (pure view retention; unmount path)
   *
   *   Routes here but deliberately KEEPS the PTY alive:
   *     - heartbeat-surfaced tabs (close-as-minimize, paths (a)/(b) below)
   *     - API-sandbox cockpit tabs (attachAgentName ≠ `tab-<id>`; the
   *       v2 close targets the tab-scoped name, so the tenant-owned cell
   *       is never killed by closing its cockpit view)
   *
   * Fire-and-forget: close failures are logged but don't block the
   * UI, matching the pattern already used for terminal_kill.
   */
  function closeTerminalForRenderer(
    data: TerminalItemData,
    opts?: { forceReap?: boolean; wholeTab?: boolean },
  ): void {
    // V23 — a whole-tab close (removeTab / removeTabFromGroup / force reap)
    // tells the daemon, so it can refuse an empty-command respawn of this
    // `tab-<pg>` from a window that has not dropped the tab yet. A pane or
    // item close inside a tab does not (the pane-group id can be reused).
    const closeReason = opts?.wholeTab ? ({ reason: 'tab_close' } as const) : {}
    // Home M4: a view-only room closes nothing on its server. Its tab
    // strip offers no close; this guards every other path (MS38 read).
    if (readOnly) {
      console.info('[tabs] view-only room — close not sent to %s (terminal %s)', scope.hostKey, data.terminalId)
      return
    }
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
    // API host-session / sandbox cockpit tabs (`api-…`) — minimize, don't
    // kill. Same contract as heartbeats: PTY stays in v2_session_map so
    // Chat history → API can re-surface the session.
    // forceReap: operator "forcefully reap" — kill PTY + drop durable
    // index so a daemon reboot cannot recovered_launch the cell.
    if (isApiOriginTerminal(data)) {
      if (opts?.forceReap) {
        const name = data.attachAgentName
        if (name) {
          closeV2Session(scope, name, { clearIndex: true, ...closeReason })
        }
        return
      }
      console.info(
        '[tabs] api-session tab close — leaving PTY alive (agent=%s, terminalId=%s)',
        data.attachAgentName ?? '(none)',
        data.terminalId,
      )
      return
    }
    if (data.heartbeatName && data.projectPath && data.surfacedAgentName) {
      daemonCliPost(scope, 'session/set-surfaced', {
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
      const entries = deps.heartbeatEntries()
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
        // /cli/terminal/kill route. (A MISSING renderer stamp lands
        // here too — matches PaneGroupView's render dispatch, where an
        // unstamped item hosts the legacy Alacritty view.)
        terminalKill(scope, data.terminalId).catch((e) =>
          console.warn('[tabs] terminal/kill failed:', e),
        )
        break
      case 'kessel':
      case 'alacritty-v2': // pre-rename working name for the Kessel stack
        // Daemon-owned PTY; unregister from v2_session_map so the
        // last Arc drops and DaemonPtySession tears down the child
        // + PTY master. See .k2so/prds/alacritty-v2.md phase A6.
        closeV2Session(scope, `tab-${data.terminalId}`, closeReason)
        break
      default:
        // Drift guard (2026-07-02 PTY-leak incident). This switch used
        // to fall through SILENTLY for any renderer value it didn't
        // know, which turns a future renderer rename/addition into a
        // permanent daemon-session leak: a daemon-owned PTY that misses
        // its A6 close outlives every client and no reaper covers bare
        // tab sessions. Fail TOWARD closing — the modern stacks are all
        // daemon-hosted, and a v2 close for a name the daemon doesn't
        // know is a logged no-op, while a missed close leaks forever.
        console.error(
          `[tabs] closeTerminalForRenderer: unknown renderer '${String(renderer)}' — issuing v2 close anyway (add the case!)`,
        )
        closeV2Session(scope, `tab-${data.terminalId}`, closeReason)
        break
    }
  }

  // ── Helpers ──────────────────────────────────────────────────────────────

  let tabCounter = 0

  /** Ensure the pinned system agent tab matches the given agent mode.
   *  Called by the projects store after workspace restore/switch.
   *  Resolves the primary agent name from the backend asynchronously. */
  function ensurePinnedAgentTabForMode(
    agentMode: string,
    projectPath: string,
  ): void {
    // Capture the workspace this call is FOR. setActiveWorkspace sets
    // activeWorkspaceKey (via restoreWorkspace) synchronously before
    // calling us, so this is the workspace the caller intends to pin
    // tabs for. We re-check it after the async agent-name resolution
    // below and bail if the user switched away in the meantime — see
    // the guard before ensureSystemAgentTabs.
    const expectedWorkspaceKey = store.getState().activeWorkspaceKey
    setTimeout(async () => {
      const tabsStore = store.getState()

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
        type AgentRow = { name: string; isManager?: boolean; agentType: string }
        // MS67 — `k2so_agents_list` reads THIS computer. A room that may not
        // call local commands asks its own server's daemon instead.
        const agents = binding.localCommands
          ? await invoke<AgentRow[]>('k2so_agents_list', { projectPath })
          : asArray<AgentRow>(await daemonCliGet(scope, 'agents/list', { project: projectPath }))
        if (agents && agents.length > 0) {
          // Find the primary agent: manager/coordinator first, then first custom, then first agent
          const manager = agents.find((a) => a.isManager || a.agentType === 'manager' || a.agentType === 'coordinator')
          const custom = agents.find((a) => a.agentType === 'custom')
          // Stage A dual-read: `k2` and legacy `k2so` are the same builtin type.
          const k2so = agents.find((a) => isBuiltinAgentType(a.agentType))
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
          agentName = await agentDisplayName(scope, projectPath)
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
      if (store.getState().activeWorkspaceKey !== expectedWorkspaceKey) return
      tabsStore.ensureSystemAgentTabs(agentName, projectPath, title)
    }, 0)
  }

  function restampBuiltTabs(tabs: Tab[], liveById: Map<string, Tab>): Tab[] {
    return tabs.map((tab) => {
      const live = liveById.get(tab.id)
      const withCid = preserveConversationIds(tab, live)
      const adopted = adoptRestoredTab(scope, withCid, live)
      if (withCid.title === adopted.title && withCid.locked === adopted.locked) return withCid
      return { ...withCid, title: adopted.title, locked: adopted.locked }
    })
  }

  /** Debounce timer for auto-saving the active workspace. */
  let persistDebounceTimer: ReturnType<typeof setTimeout> | null = null

  // ── Layout-save durability (0.40.48 — the remote split-columns bug) ──────
  //
  // The canonical layout save was a debounced, SINGLE-SHOT, fire-and-forget
  // POST: any failure was console.error'd and the layout change was simply
  // gone from the server. Locally that never bites (the loopback save can't
  // fail), but against a remote host one dropped save — a tunnel blip during
  // the 1s debounce, a transient non-2xx, or the 0.40.48 recovery gate
  // failing fast with RecoveringError — meant a just-clicked column split
  // never reached the remote DB, and the NEXT layout read (host switch,
  // remote-reorder refetch, reconnect re-restore) clobbered the live
  // `splitCount` back to the stale server copy: "the split doesn't hold."
  //
  // The re-arm below makes the save durable: a RecoveringError parks on the
  // recovery signal and flushes the moment the host is back; other failures
  // retry on a jittered timer with a consecutive-failure cap (each retry
  // re-serializes CURRENT state, so collapsing bursts is correct —
  // last-write-wins). One pending re-arm at a time.
  let layoutSaveRetryTimer: ReturnType<typeof setTimeout> | null = null
  let layoutSaveRecoveryWait: (() => void) | null = null
  let layoutSaveConsecutiveFailures = 0

  function rearmLayoutSave(err: unknown): void {
    if (layoutSaveRetryTimer !== null || layoutSaveRecoveryWait !== null) return
    // Host switch already cancelled timers; do not persist a leftover
    // strip (or a half-restored one) onto the NEW host.
    if (err instanceof Error && err.name === 'HostSwitchedError') return
    if (err instanceof RecoveringError) {
      // The gate dropped the save before sending — the host is recovering.
      // Flush as soon as it's genuinely back (push-style, nothing polls).
      layoutSaveRecoveryWait = onceRecovered(scope, () => {
        layoutSaveRecoveryWait = null
        store.getState().persistActiveWorkspace()
      })
      return
    }
    // Blind (non-recovery) retries are REMOTE-ONLY: a loopback save can't
    // blip — a local failure is deterministic (bug/broken env), and
    // retrying it just churns timers (and pollutes unit-test envs, where
    // every save fails by construction). The remote tunnel path is where
    // transient drops actually happen.
    if (!scope.isRemote) return
    layoutSaveConsecutiveFailures += 1
    if (layoutSaveConsecutiveFailures > LAYOUT_SAVE_MAX_BLIND_RETRIES) {
      // Persistent non-recovery failure — stop churning; the next structural
      // change (or recovery flip) re-triggers a save naturally.
      return
    }
    layoutSaveRetryTimer = setTimeout(() => {
      layoutSaveRetryTimer = null
      store.getState().persistActiveWorkspace()
    }, jittered(LAYOUT_SAVE_RETRY_BASE_MS))
  }

  /** A save landed — clear the failure streak. */
  function layoutSaveSucceeded(): void {
    layoutSaveConsecutiveFailures = 0
  }

  /** The (former) debounce body of `persistActiveWorkspace`, extracted so
   *  `flushLayoutPersist` can run it immediately for mutations that must
   *  hold (0.40.48 — column split/unsplit). Serializes CURRENT state at
   *  call time; failures re-arm via `rearmLayoutSave`. */
  function saveActiveWorkspaceLayoutNow(opts?: { allowEmpty?: boolean }): Promise<void> {
    if (isLayoutSaveSuppressed()) return Promise.resolve()
    const state = store.getState()
    const key = state.activeWorkspaceKey
    if (!key) return Promise.resolve()
    // An empty strip is saved only by an explicit close (V16/V22). A general
    // autosave with no tabs is a mid-switch view-clear, not the user's layout.
    if (!opts?.allowEmpty && state.tabs.length === 0 && state.extraGroups.length === 0) {
      return Promise.resolve()
    }
    const [projectId, workspaceId] = key.split(':')
    if (!projectId || !workspaceId) return Promise.resolve()
    return submitLayoutSave(key, activeLayoutSaveJob(key, projectId, workspaceId, {
      allowEmpty: opts?.allowEmpty,
      label: 'Auto-save',
    }))
  }

  /** A save job for the open workspace: serializes this window's state at send
   *  time, merges on a 409, and keeps the cached copy in step. */
  function activeLayoutSaveJob(
    key: string,
    projectId: string,
    workspaceId: string,
    opts: { allowEmpty?: boolean; force?: boolean; label: string },
  ): LayoutSaveJob {
    return {
      projectId,
      workspaceId,
      conflict: 'merge',
      retry: true,
      force: opts.force,
      label: opts.label,
      produce: () => {
        if (isLayoutSaveSuppressed()) return null
        const st = store.getState()
        if (st.activeWorkspaceKey !== key) return null
        const empty = st.tabs.length === 0 && st.extraGroups.length === 0
        if (empty && !opts.allowEmpty) return null
        const layout = empty ? EMPTY_SERIALIZED_LAYOUT : st.serializeCurrentLayout()
        if (empty) {
          const { [key]: _gone, ...remaining } = st.workspaceLayouts
          store.setState({ workspaceLayouts: remaining })
          cacheRevisions.delete(key)
        } else {
          store.setState({ workspaceLayouts: { ...st.workspaceLayouts, [key]: layout } })
        }
        return layout
      },
    }
  }

  /** Cancel a pending debounced autosave (see `persistActiveWorkspace`). */
  function cancelPendingLayoutSave(): void {
    if (persistDebounceTimer) {
      clearTimeout(persistDebounceTimer)
      persistDebounceTimer = null
    }
  }

  /** Host-switch: debounce + the remote-save retry/recovery wait. */
  function cancelLayoutPersistForHostSwitch(): void {
    cancelPendingLayoutSave()
    if (layoutSaveRetryTimer !== null) {
      clearTimeout(layoutSaveRetryTimer)
      layoutSaveRetryTimer = null
    }
    if (layoutSaveRecoveryWait !== null) {
      layoutSaveRecoveryWait()
      layoutSaveRecoveryWait = null
    }
  }

  /** Find a tab across all groups (group 0 = main tabs, groups 1+ = extraGroups) */
  /** When column 0 empties while a later column still has tabs (a close or a
   *  move of its last tab), move the columns left. An empty column 0 reads as
   *  "no layout" to every reader of the shared layout. */
  function collapseEmptyLeadingColumnsInStore(): void {
    const st = store.getState()
    if (st.tabs.length > 0) return
    if (!st.extraGroups.some((g) => g.tabs.length > 0)) return
    let tabs = st.tabs
    let activeTabId = st.activeTabId
    let groups = st.extraGroups.slice()
    let splitCount = st.splitCount
    let activeGroupIndex = st.activeGroupIndex
    while (tabs.length === 0 && groups.some((g) => g.tabs.length > 0)) {
      const [next, ...rest] = groups
      tabs = next.tabs
      activeTabId = next.activeTabId ?? next.tabs[0]?.id ?? null
      groups = rest
      splitCount = Math.max(1, splitCount - 1)
      activeGroupIndex = Math.max(0, activeGroupIndex - 1)
    }
    store.setState({ tabs, activeTabId, extraGroups: groups, splitCount, activeGroupIndex })
  }

  /** Restore one serialized tab — the same rules for column 0 and the split
   *  columns. Tab and pane-group ids are the saved ones: the daemon's
   *  `tab-<paneGroupId>` session is keyed by the pane-group id, so reusing it
   *  re-attaches the restored pane to the live PTY. */
  function restoreSerializedTab(
    serializedTab: SerializedTab,
    cwd: string,
    liveTabsById: Map<string, Tab>,
    liveItemsById: Map<string, Item>,
    usedItemIds: Set<string>,
  ): Tab {
    const paneGroups = new Map<string, PaneGroup>()
    const idMap = new Map<string, string>()
    // Handle both new format (paneGroups) and legacy format (panes)
    const serializedPaneGroups = serializedTab.paneGroups
      ?? convertLegacyPanes((serializedTab as unknown as { panes?: Record<string, LegacySerializedPaneData> }).panes)
    if (!serializedPaneGroups || typeof serializedPaneGroups !== 'object') {
      console.warn('[tabs] Corrupted layout: missing paneGroups, creating fresh tab')
      const pgId = crypto.randomUUID()
      paneGroups.set(pgId, makeTerminalPaneGroup(pgId, cwd))
      idMap.set('default', pgId)
    } else {
      for (const [oldPgId, serializedPg] of Object.entries(serializedPaneGroups)) {
        const newPgId = oldPgId
        idMap.set(oldPgId, newPgId)
        const rawItems = Array.isArray(serializedPg?.items) ? serializedPg.items : []
        const items: Item[] = rawItems.map((si) =>
          restoreSerializedItem(si, newPgId, cwd, liveItemsById, usedItemIds),
        )
        // Ensure at least one item per pane group. A stable id, so every
        // window (and every restore) makes the same one.
        if (items.length === 0) {
          const fallbackId = usedItemIds.has(`item-${newPgId}`) ? crypto.randomUUID() : `item-${newPgId}`
          usedItemIds.add(fallbackId)
          items.push({
            id: fallbackId,
            type: 'terminal',
            data: { terminalId: newPgId, cwd, renderer: currentRenderer() },
          })
        }
        const clampedIndex = Math.max(0, Math.min(serializedPg?.activeItemIndex ?? 0, items.length - 1))
        paneGroups.set(newPgId, { id: newPgId, items, activeItemIndex: clampedIndex })
      }
    }
    tabCounter++
    // Reuse the daemon-canonical serialized id (fall back for legacy layouts
    // saved before ids were serialized). Re-minting here broke cross-client
    // tab identity: TabTitleChanged events + tab_titles snapshots key on the
    // RENAMER's id (the remote-rename-invisible bug, 2026-07-08).
    const id = serializedTab.id ?? crypto.randomUUID()
    const live = liveTabsById.get(id)
    return {
      id,
      title: serializedTab.title,
      mosaicTree: remapMosaicIds(serializedTab.mosaicTree, idMap),
      paneGroups,
      ...(serializedTab.isSystemAgent ? { isSystemAgent: true } : {}),
      ...(serializedTab.isPinnedFile ? { isPinnedFile: true } : {}),
      // Tab-rename stickiness — restore the locked flag so a user-renamed tab
      // stays sticky after relaunch.
      ...(serializedTab.locked ? { locked: true } : {}),
      // Unsaved edits are this window's; a remote restore must not clear them.
      ...(live?.isDirty ? { isDirty: true } : {}),
    }
  }

  // ── Store ────────────────────────────────────────────────────────────────

  const store = create<TabsState>((set, get) => ({
    tabs: [],
    activeTabId: null,
    splitCount: 1,
    extraGroups: [],
    activeGroupIndex: 0,
    navHistory: [],
    navIndex: -1,

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

    addTab: (cwd: string, options?: AddTerminalTabOptions) => {
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
      // conversationId is a named chat — lock the display name so PTY/harness
      // labels cannot unlocked-write the strip (same as restampSessionTabs).
      const lockTitle = Boolean(options?.locked || options?.conversationId?.trim())

      const tab: Tab = {
        id: tabId,
        title,
        mosaicTree: paneGroupId,
        paneGroups: new Map([[paneGroupId, paneGroup]]),
        ...(lockTitle ? { locked: true } : {}),
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

    removeTab: (tabId: string, opts?: { forceReap?: boolean }) => {
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
              closeTerminalForRenderer(data, { ...opts, wholeTab: true })
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
      collapseEmptyLeadingColumnsInStore()

      // 0.38.0 Commit 4 — no cross-window broadcast here. The daemon's
      // `/cli/sessions/events` push delivers `session_removed` to other
      // windows after `closeTerminalForRenderer` unregisters the v2 PTY.
      //
      // V22 — a close is saved at once, not on the 1s debounce, so another
      // window's save in that second can't carry the closed tab back.
      get().flushLayoutPersist({ allowEmpty: true })
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

    releasePaneOwnedElsewhere: (paneId) => {
      dropPaneOwnedElsewhere(paneId)
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
        : deps.activeProjectId()
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
    ): Promise<string | null> => {
      // Pinned chat is the caller's branch (drawer row and open icon)
      // before this runs. Pinned mode leaves last_session_id set, so a
      // conversation search here would focus that leftover chat.
      //
      // 1. Conversation id on the main strip and every extra strip
      //    (Chat history's walk). findChatSessionInTab skips heartbeat
      //    surfaces; a second pass focuses a tab we already attached.
      // 2. A live PTY with no tab is attached by its v2 map name
      //    (`<project>:hb:<name>` or `tab-*`). Do not post
      //    session/set-surfaced — that flag is one bit for the whole
      //    project, and the listener would append a companion.
      // 3. Cold: the id active-session already returned, else the host
      //    heartbeat list. Never the local k2so_heartbeat_list invoke.
      const nonempty = (id: string | null | undefined): string | null => {
        if (typeof id !== 'string') return null
        const trimmed = id.trim()
        return trimmed.length > 0 ? trimmed : null
      }

      const focusAt = (groupIndex: number, tabId: string): string => {
        if (groupIndex === 0) get().setActiveTab(tabId)
        else get().setActiveTabInGroup(groupIndex, tabId)
        return tabId
      }

      const strips = (): Array<{ tabs: Tab[]; idx: number }> => {
        const state = get()
        return [
          { tabs: state.tabs, idx: 0 },
          ...state.extraGroups.map((g, i) => ({ tabs: g.tabs, idx: i + 1 })),
        ]
      }

      const focusOpenSession = (
        sessionId: string | null,
        activeTerminalId: string | null,
        activeAgentName: string | null,
      ): string | null => {
        const ids = sessionId ? new Set([sessionId]) : null
        if (ids) {
          for (const { tabs, idx } of strips()) {
            for (const tab of tabs) {
              if (findChatSessionInTab(tab, ids)) return focusAt(idx, tab.id)
            }
          }
        }
        const rendererId = activeAgentName?.startsWith('tab-')
          ? activeAgentName.slice('tab-'.length)
          : null
        for (const { tabs, idx } of strips()) {
          for (const tab of tabs) {
            if (tab.isSystemAgent) continue
            for (const [, pg] of tab.paneGroups) {
              for (const item of pg.items) {
                if (item.type !== 'terminal') continue
                const td = item.data as TerminalItemData
                if (td.fromApi) continue
                if (ids && td.conversationId?.trim() && ids.has(td.conversationId.trim())) {
                  return focusAt(idx, tab.id)
                }
                if (ids && td.heartbeatName && td.args?.some((arg) => ids.has(arg))) {
                  return focusAt(idx, tab.id)
                }
                if (activeAgentName && td.attachAgentName === activeAgentName) {
                  return focusAt(idx, tab.id)
                }
                if (rendererId && td.terminalId === rendererId) {
                  return focusAt(idx, tab.id)
                }
                if (activeTerminalId && td.terminalId === activeTerminalId) {
                  return focusAt(idx, tab.id)
                }
              }
            }
          }
        }
        return null
      }

      let active: {
        name: string
        claudeSessionId: string | null
        activeTerminalId: string | null
        activeAgentName: string | null
        sessionAlive: boolean
        isV2: boolean
      } | null = null
      try {
        // Host-aware: the live PTY is on the ACTIVE host. A local
        // invoke would miss a remote heartbeat and fall through to spawn.
        const raw = await daemonCliGetText(scope, 'heartbeat/active-session', {
          project: projectPath,
          name: heartbeatName,
        })
        active = JSON.parse(raw)
      } catch (err) {
        console.warn('[openHeartbeatTab] active-session lookup failed:', err)
      }

      const sessionFromActive = nonempty(active?.claudeSessionId)
      const live = !!(active?.sessionAlive && nonempty(active.activeTerminalId))
      const mapName = live ? nonempty(active?.activeAgentName) : null
      const liveTerminalId = live ? nonempty(active?.activeTerminalId) : null

      const focused = focusOpenSession(sessionFromActive, liveTerminalId, mapName)
      if (focused) return focused

      if (live && liveTerminalId && mapName) {
        // Same tab shape the session:surfaced listener builds, so
        // TerminalPane attaches via attachAgentName instead of spawning.
        const launch = await resolveSessionResumeLaunch(scope, projectPath, sessionFromActive)
        let surfacedAgentName: string | undefined
        try {
          const agents = asArray<{ name: string; agentType: string }>(
            await daemonCliGet(scope, 'agents/list', { project: projectPath }).catch(() => []),
          )
          surfacedAgentName = agents.find((a) =>
            a.agentType === 'custom' || a.agentType === 'manager' || isBuiltinAgentType(a.agentType),
          )?.name ?? agents[0]?.name
        } catch { /* the map name is what reuses the PTY */ }

        const cmd = launch.command.split(' ')[0] || 'shell'
        const tabId = crypto.randomUUID()
        const paneGroupId = liveTerminalId
        const itemId = crypto.randomUUID()
        const newTab: Tab = {
          id: tabId,
          title: `${heartbeatName} (heartbeat)`,
          isSystemAgent: false,
          paneGroups: new Map([
            [
              paneGroupId,
              {
                id: paneGroupId,
                items: [
                  {
                    id: itemId,
                    type: 'terminal',
                    data: {
                      terminalId: liveTerminalId,
                      cwd: projectPath,
                      command: cmd,
                      args: launch.args,
                      renderer: 'kessel',
                      spawnedAt: performance.now(),
                      heartbeatName,
                      projectPath,
                      surfacedAgentName,
                      attachAgentName: mapName,
                    },
                  },
                ],
                activeItemIndex: 0,
              },
            ],
          ]),
          mosaicTree: paneGroupId,
        }
        set((s) => ({
          tabs: [...s.tabs, newTab],
          activeTabId: tabId,
        }))
        return tabId
      }

      if (live) {
        // Alive, but no map name to attach by. A fresh spawn would be
        // a second process against the session that is already running.
        console.warn('[openHeartbeatTab] live PTY has no map name; not spawning a second process')
        return null
      }

      let sessionId = sessionFromActive
      if (!sessionId) {
        const listed = asArray<{ name: string; lastSessionId?: string | null }>(
          await daemonCliGet(scope, 'heartbeat/list', { project: projectPath }).catch(() => []),
        )
        const hb = listed.find((row) => row.name === heartbeatName)
        if (!hb) {
          console.warn('[openHeartbeatTab] heartbeat not found:', heartbeatName)
          return null
        }
        sessionId = nonempty(hb.lastSessionId ?? null)
        if (!sessionId) {
          console.info('[openHeartbeatTab] heartbeat has no saved session yet; click Launch first')
          return null
        }
        const again = focusOpenSession(sessionId, null, null)
        if (again) return again
      }

      const launch = await resolveSessionResumeLaunch(scope, projectPath, sessionId)
      if (launch.provider === 'claude') {
        const jsonlExists = await daemonCliGet<{ exists: boolean }>(scope, 'chat/session-exists', {
          project_path: projectPath,
          session_id: sessionId,
        })
          .then((r) => r.exists)
          .catch(() => true)
        if (!jsonlExists) {
          console.info(
            '[openHeartbeatTab] saved session %s has no JSONL on disk yet; click Launch to refire',
            sessionId,
          )
          return null
        }
      }

      const targetGroup = get().splitCount > 1 ? get().splitCount - 1 : 0
      const paneGroupId = get().addTabToGroup(targetGroup, projectPath, {
        title: `${heartbeatName} (heartbeat)`,
        command: launch.command,
        args: launch.args,
        conversationId: sessionId,
      })
      const created = [...get().tabs, ...get().extraGroups.flatMap((g) => g.tabs)]
        .find((t) => t.paneGroups.has(paneGroupId))
      if (!created) return null

      // Heartbeat PTYs live in v2_session_map, so the tab must use Kessel
      // even when the workspace renderer preference is legacy.
      const stampKessel = (tab: Tab): Tab => {
        if (tab.id !== created.id) return tab
        const updatedGroups = new Map(tab.paneGroups)
        for (const [pgId, pg] of updatedGroups) {
          updatedGroups.set(pgId, {
            ...pg,
            items: pg.items.map((item) => {
              if (item.type !== 'terminal') return item
              return { ...item, data: { ...item.data, renderer: 'kessel' as const } }
            }),
          })
        }
        return { ...tab, paneGroups: updatedGroups }
      }
      set((s) => ({
        tabs: s.tabs.map(stampKessel),
        extraGroups: s.extraGroups.map((g) => ({ ...g, tabs: g.tabs.map(stampKessel) })),
      }))
      return created.id
    },

    ensureSystemAgentTabs: (agentName: string, projectPath: string, _title: string) => {
      const state = get()

      // The split landed in 0.36.0: each agent gets up to two pinned tabs.
      // Canonical order is Chat first, Inbox second — fixed regardless of
      // creation history. Workspace-board mode (`__workspace__`) is the
      // exception — it has no chat surface, just the Inbox/Work Board
      // pane — so we create only the Inbox tab in that case.
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

    openUrlInNewTab: (url: string, groupIndex?: number) => {
      const paneGroupId = crypto.randomUUID()
      const tabId = crypto.randomUUID()

      const pg = makeBrowserPaneGroup(paneGroupId, url)

      const tab: Tab = {
        id: tabId,
        title: browserTabTitle(url),
        mosaicTree: paneGroupId,
        paneGroups: new Map([[paneGroupId, pg]])
      }

      set((state) => placedTab(state, groupIndex ?? 0, tab))
    },

    openUrlInPane: (tabId: string, url: string) => {
      set((state) => {
        const result = mapTabAcrossGroups(state, tabId, (tab) => {
          const activePgId = getFirstLeaf(tab.mosaicTree)
          if (!activePgId) return tab
          const pg = tab.paneGroups.get(activePgId)
          if (!pg) return tab

          const newPaneGroups = new Map(tab.paneGroups)

          // Reuse an existing unpinned browser item — navigate in place
          // (BrowserPane reacts to the url data change) — same reuse rule
          // as openFileInPane's unpinned file-viewer.
          const browserIdx = pg.items.findIndex(
            (item) => item.type === 'browser' && !item.pinned
          )
          if (browserIdx !== -1) {
            const newItems = [...pg.items]
            const prev = newItems[browserIdx]
            const prevData = prev.data as BrowserItemData
            // V21 — this window asked for the navigation; bump navSeq so the
            // pane moves its live page (a url change alone never does).
            newItems[browserIdx] = {
              ...prev,
              data: { ...prevData, url, navSeq: (prevData.navSeq ?? 0) + 1 },
            }
            newPaneGroups.set(activePgId, { ...pg, items: newItems, activeItemIndex: browserIdx })
          } else {
            const newItem: Item = {
              id: crypto.randomUUID(),
              type: 'browser',
              data: { url },
            }
            const newItems = [...pg.items, newItem]
            newPaneGroups.set(activePgId, { ...pg, items: newItems, activeItemIndex: newItems.length - 1 })
          }

          return { ...tab, paneGroups: newPaneGroups }
        })
        return { tabs: result.tabs, extraGroups: result.extraGroups }
      })
    },

    setBrowserItemState: (tabId: string, paneGroupId: string, itemId: string, browserState: { url?: string; title?: string }) => {
      set((state) => {
        // V8/V19 — a same-value stamp (a remounted webview reporting the URL
        // the layout already has) changes nothing and must not save.
        let changed = false
        const result = mapTabAcrossGroups(state, tabId, (tab) => {
          const pg = tab.paneGroups.get(paneGroupId)
          if (!pg) return tab
          let pgChanged = false
          const newItems = pg.items.map((item) => {
            if (item.id !== itemId || item.type !== 'browser') return item
            const prevData = item.data as BrowserItemData
            const same =
              (browserState.url === undefined || browserState.url === prevData.url) &&
              (browserState.title === undefined || browserState.title === prevData.title)
            if (same) return item
            pgChanged = true
            return { ...item, data: { ...prevData, ...browserState } }
          })
          if (!pgChanged) return tab
          changed = true
          const newPaneGroups = new Map(tab.paneGroups)
          newPaneGroups.set(paneGroupId, { ...pg, items: newItems })
          return { ...tab, paneGroups: newPaneGroups }
        })
        if (!changed) return {}
        return { tabs: result.tabs, extraGroups: result.extraGroups }
      })
    },

    applyBrowserPageMeta: (tabId, paneGroupId, itemId, meta) => {
      set((state) => {
        let changed = false
        const result = mapTabAcrossGroups(state, tabId, (tab) => {
          const next = withBrowserPageMeta(tab, paneGroupId, itemId, meta)
          if (next !== tab) changed = true
          return next
        })
        if (!changed) return {}
        return { tabs: result.tabs, extraGroups: result.extraGroups }
      })
    },

    openUntitledDocument: (cwd: string, groupIndex?: number) => {
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

      set((state) => placedTab(state, groupIndex ?? 0, tab))
    },

    setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => {
      const locked = opts?.locked === true
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
      // Tab-rename stickiness — the user's rename wins. An AUTO path (PTY/OSC
      // title listener, daemon `label_changed`) calls without `locked`; if the
      // tab is already locked by a prior user rename, SKIP — a program-
      // generated "✳ Claude Code" title must never snap back over the user's
      // chosen label. A user rename (`locked: true`) always proceeds and (re)
      // locks the tab.
      if (!locked && target?.locked) {
        return
      }
      if (!target) return
      const adopted = adoptTabTitle(
        {
          title: target.title,
          locked: target.locked,
          conversationId: conversationIdFromTab(target),
        },
        {
          title,
          locked: opts?.locked,
          conversationId: conversationIdFromTab(target),
        },
      )
      if (target.title === adopted.title && (target.locked === true) === adopted.locked) return
      set((state) => {
        const result = mapTabAcrossGroups(state, tabId, (tab) => ({
          ...tab,
          title: adopted.title,
          locked: adopted.locked,
        }))
        return { tabs: result.tabs, extraGroups: result.extraGroups }
      })
      // 0.39.39 (#676) — tab titles are now daemon-canonical so a rename in
      // one client/window converges on every connected client + the mobile
      // companion. Persist to the daemon-owned `tab_titles` store (which
      // broadcasts `TabTitleChanged`); the local-layout persistence (via the
      // auto-save debounce) stays as the fallback when the daemon doesn't
      // advertise the broadcast capability. The store is keyed by
      // (projectId, tabId), so we send the active workspace's projectId.
      // `locked` rides the POST so the daemon marks a USER rename sticky (and
      // stops the renderer's own auto-pushed PTY titles from clobbering it).
      if (scope.serverSupports('daemon-broadcasts')) {
        const projectId = get().activeWorkspaceKey?.split(':')[0]
        if (projectId) {
          daemonCliPost(scope, 'workspace/set-tab-title', {
            projectId,
            tabId,
            title: adopted.title,
            locked: adopted.locked,
          }).catch((err) => {
            console.error('[tabs] set-tab-title persist failed:', err)
          })
        }
      }
    },

    setTerminalSandboxBackend: (terminalId: string, backend: string | undefined) => {
      // D9 — stamp the resolved backend name from the spawn response onto
      // the matching terminal item's `data.sandboxBackend`. Scans every
      // tab's paneGroups across both groups. Uses the immutable shallow-
      // copy pattern (mirror of reconcileSystemAgentTab) so Zustand
      // reference-equality selectors notice the change — NEVER mutate in
      // place. No-ops (returns the same references) when nothing matches
      // or the value is unchanged, so it can't trigger spurious renders.
      set((state) => {
        let anyTabChanged = false
        const patchTab = (tab: Tab): Tab => {
          let tabChanged = false
          const newPaneGroups = new Map<string, PaneGroup>()
          for (const [pgId, pg] of tab.paneGroups) {
            let pgChanged = false
            const newItems = pg.items.map((item) => {
              if (item.type !== 'terminal') return item
              const d = item.data as TerminalItemData
              if (d.terminalId !== terminalId) return item
              if (d.sandboxBackend === backend) return item
              pgChanged = true
              return { ...item, data: { ...d, sandboxBackend: backend } }
            })
            if (pgChanged) tabChanged = true
            newPaneGroups.set(pgId, pgChanged ? { ...pg, items: newItems } : pg)
          }
          if (!tabChanged) return tab
          anyTabChanged = true
          return { ...tab, paneGroups: newPaneGroups }
        }
        const newTabs = state.tabs.map(patchTab)
        const newExtraGroups = state.extraGroups.map((g) => {
          const tabs = g.tabs.map(patchTab)
          return tabs === g.tabs ? g : { ...g, tabs }
        })
        if (!anyTabChanged) return {}
        return { tabs: newTabs, extraGroups: newExtraGroups }
      })
    },

    setTerminalConversationId: (terminalId: string, conversationId: string | undefined) => {
      const next = conversationId?.trim() || undefined
      set((state) => {
        let anyTabChanged = false
        const patchTab = (tab: Tab): Tab => {
          let tabChanged = false
          const newPaneGroups = new Map<string, PaneGroup>()
          for (const [pgId, pg] of tab.paneGroups) {
            let pgChanged = false
            const newItems = pg.items.map((item) => {
              if (item.type !== 'terminal') return item
              const d = item.data as TerminalItemData
              if (d.terminalId !== terminalId) return item
              if (d.conversationId === next) return item
              pgChanged = true
              return { ...item, data: { ...d, conversationId: next } }
            })
            if (pgChanged) tabChanged = true
            newPaneGroups.set(pgId, pgChanged ? { ...pg, items: newItems } : pg)
          }
          if (!tabChanged) return tab
          anyTabChanged = true
          return { ...tab, paneGroups: newPaneGroups }
        }
        const newTabs = state.tabs.map(patchTab)
        const newExtraGroups = state.extraGroups.map((g) => {
          const tabs = g.tabs.map(patchTab)
          return tabs === g.tabs ? g : { ...g, tabs }
        })
        if (!anyTabChanged) return {}
        return { tabs: newTabs, extraGroups: newExtraGroups }
      })
    },

    applyDaemonTabTitle: (tabId: string, title: string, locked?: boolean) => {
      // Local-only apply (no re-POST) for the broadcast handler + on-load
      // snapshot. Same pinned-system-agent guard as setTabTitle: those tabs
      // own their functional bar labels and ignore canonical renames.
      const allTabs = (() => {
        const s = get()
        return [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
      })()
      const target = allTabs.find((t) => t.id === tabId)
      if (!target || target.isSystemAgent) return
      // A local lock (Chats custom_name / user rename) must not be
      // unlocked-written by PTY label_initial, OSC, or a snapshot that
      // omitted `locked`. Remote USER renames still apply (`locked: true`).
      if (target.locked && locked !== true) return
      const adopted = adoptTabTitle(
        {
          title: target.title,
          locked: target.locked,
          conversationId: conversationIdFromTab(target),
        },
        {
          title,
          locked,
          conversationId: conversationIdFromTab(target),
        },
      )
      if (target.title === adopted.title && (target.locked === true) === adopted.locked) return
      set((state) => {
        const result = mapTabAcrossGroups(state, tabId, (tab) => ({
          ...tab,
          title: adopted.title,
          locked: adopted.locked,
        }))
        return { tabs: result.tabs, extraGroups: result.extraGroups }
      })
    },

    renameTabByTitle: (oldTitle: string, newTitle: string) => {
      set((state) => {
        const updateTab = (tab: Tab) => {
          if (tab.title !== oldTitle) return tab
          const adopted = adoptTabTitle(
            {
              title: tab.title,
              locked: tab.locked,
              conversationId: conversationIdFromTab(tab),
            },
            { title: newTitle, conversationId: conversationIdFromTab(tab) },
          )
          return { ...tab, title: adopted.title, locked: adopted.locked }
        }
        return {
          tabs: state.tabs.map(updateTab),
          extraGroups: state.extraGroups.map((group) => ({
            ...group,
            tabs: group.tabs.map(updateTab),
          })),
        }
      })
    },

    setTabDirty: (tabId: string, dirty: boolean) => {
      set((state) => {
        // V19 — FileViewerPane calls this on every mount; an unchanged flag
        // must not make a new tabs array (that fired the autosave).
        const current = findTabAcrossGroups(state, tabId)
        if (!current || Boolean(current.isDirty) === dirty) return {}
        const result = mapTabAcrossGroups(state, tabId, (tab) => ({ ...tab, isDirty: dirty }))
        return { tabs: result.tabs, extraGroups: result.extraGroups }
      })
    },

    // ── Unsaved-changes leave guard support ──────────────────────────────
    // `projectId` is unused: the store only ever holds the ACTIVE workspace's
    // tabs (group 0 `tabs` + `extraGroups`), so the in-view dirty tabs ARE the
    // leaving workspace's unsaved tabs. The param keeps call sites self-
    // documenting and lets a future multi-workspace store filter by owner.
    dirtyTabIdsForProject: (_projectId: string) => {
      const s = get()
      const allTabs = [...s.tabs, ...s.extraGroups.flatMap((g) => g.tabs)]
      return allTabs.filter((t) => t.isDirty).map((t) => t.id)
    },

    markTabsDiscardPending: (tabIds: string[]) => {
      for (const id of tabIds) _discardPendingTabs.add(id)
    },

    consumeDiscardPending: (tabId: string) => {
      if (_discardPendingTabs.has(tabId)) {
        _discardPendingTabs.delete(tabId)
        return true
      }
      return false
    },

    setFileViewerState: (tabId: string, paneId: string, itemId: string, viewerState: { scrollTop?: number; cursorPos?: number }) => {
      set((state) => {
        // V19 — FileViewerPane writes this on every unmount; the same scroll /
        // cursor it already has is not a change.
        let changed = false
        const result = mapTabAcrossGroups(state, tabId, (tab) => {
          const pg = tab.paneGroups.get(paneId)
          if (!pg) return tab
          let pgChanged = false
          const newItems = pg.items.map((item) => {
            if (item.id !== itemId || item.type !== 'file-viewer') return item
            const prevData = item.data as FileViewerItemData
            const same =
              (viewerState.scrollTop === undefined || viewerState.scrollTop === prevData.scrollTop) &&
              (viewerState.cursorPos === undefined || viewerState.cursorPos === prevData.cursorPos)
            if (same) return item
            pgChanged = true
            return { ...item, data: { ...prevData, ...viewerState } }
          })
          if (!pgChanged) return tab
          changed = true
          const newPaneGroups = new Map(tab.paneGroups)
          newPaneGroups.set(paneId, { ...pg, items: newItems })
          return { ...tab, paneGroups: newPaneGroups }
        })
        if (!changed) return {}
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
      // 0.40.48: a split must HOLD against a remote host — save immediately
      // instead of riding the 1s debounce, so a competing TabOrderChanged
      // can't refetch the pre-split layout while the save is still queued
      // (and a failed save re-arms instead of silently dropping).
      get().flushLayoutPersist()
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
      // 0.40.48: same immediate-save rationale as splitTerminalArea.
      get().flushLayoutPersist()
    },

    setActiveGroup: (index: number) => {
      set({ activeGroupIndex: index })
    },

    addTabToGroup: (groupIndex: number, cwd: string, options?: AddTerminalTabOptions): string => {
      tabCounter++
      const tabId = crypto.randomUUID()
      const pgId = crypto.randomUUID()
      const pg = makeTerminalPaneGroup(pgId, cwd, options)
      const title = options?.title
        ?? (options?.command ? options.command.split('/').pop()?.split(' ')[0] ?? `Terminal ${tabCounter}` : `Terminal ${tabCounter}`)
      const lockTitle = Boolean(options?.locked || options?.conversationId?.trim())

      const tab: Tab = {
        id: tabId,
        title,
        mosaicTree: pgId,
        paneGroups: new Map([[pgId, pg]]),
        ...(lockTitle ? { locked: true } : {}),
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

    removeTabFromGroup: (groupIndex: number, tabId: string, opts?: { forceReap?: boolean }) => {
      if (groupIndex === 0) {
        get().removeTab(tabId, opts)
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
              closeTerminalForRenderer(item.data as TerminalItemData, { ...opts, wholeTab: true })
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
      // V22 — same immediate save as removeTab.
      get().flushLayoutPersist({ allowEmpty: true })
    },

    forceReapAllTabsInGroup: (groupIndex: number) => {
      const { tabs } = get().getGroupTabs(groupIndex)
      for (const tab of tabs) {
        if (tab.isSystemAgent) continue
        get().removeTabFromGroup(groupIndex, tab.id, { forceReap: true })
      }
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
      collapseEmptyLeadingColumnsInStore()
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
      // per-client-view-state.md (Phase 1) — the canonical layout carries only
      // STRUCTURE. `activeTabId` (top-level AND per-split-group) is per-client
      // VIEW state and is intentionally OMITTED so a save can't drag this
      // client's selection onto peers that adopt the layout. D2 — the focused
      // column (`activeGroupIndex`) is per-window too. D1 — the columns and
      // which tab sits in which column ARE shared.
      return serializeColumnsLayout(state.tabs, state.extraGroups, state.splitCount)
    },

    restoreLayout: (layout: SerializedLayout, cwd: string) => {
      try {
      // T10/T11 — snapshot live names before rebuild (and before any
      // caller clearAllTabs). Restamp of the built array is sync.
      rememberLiveNamedChatTitles(scope, collectStoreTabs(get()))
      const liveById = new Map<string, Tab>()
      for (const t of collectStoreTabs(get())) liveById.set(t.id, t)

      // 0.38.0 — v1→v2 migration. Walks all paneGroup items and drops
      // daemon-owned fields from terminal items. Daemon-owned data is
      // refilled by `reconcileWithDaemon` after restoration. The on-disk
      // copy converges to v2 on the next `saveLayoutForWorkspace` call,
      // which `loadLayoutForWorkspace` triggers when migration happens.
      migrateLayoutToV2(layout)

      // V3/V20 — restore keeps the saved item ids, so a remote save never
      // remounts a pane whose tab is still there (panes key on `item.id`). A
      // live item with the same id keeps its runtime-only data.
      const liveItemsById = new Map<string, Item>()
      for (const t of liveById.values()) {
        for (const pg of t.paneGroups.values()) {
          for (const it of pg.items) liveItemsById.set(it.id, it)
        }
      }
      const usedItemIds = new Set<string>()
      const restoreTab = (serializedTab: SerializedTab): Tab =>
        restoreSerializedTab(serializedTab, cwd, liveById, liveItemsById, usedItemIds)

      const restoredTabs: Tab[] = (Array.isArray(layout.tabs) ? layout.tabs : []).map(restoreTab)

      // per-client-view-state.md (Phase 1+2) — selection is NO LONGER read from
      // the shared layout's leaked `activeTabId` (that hijacked peers). A tab
      // this window already has selected stays selected; otherwise it comes
      // from this client's per-client selected-tabs store, matched by paneGroup
      // SIGNATURE. Fallbacks: the saved selection's tab no longer exists → first
      // tab; a brand-new client with no saved selection → first tab (preserves
      // #658 cold-boot pinned-chat, which is the first/system tab).
      const liveSelected = get().activeTabId
      const restoredActiveTabId =
        liveSelected && restoredTabs.some((t) => t.id === liveSelected)
          ? liveSelected
          : resolveRestoredSelection(get(), restoredTabs)

      // Restore extra groups (split columns). D1 — columns and their tabs are
      // shared. D2 — each column's selected tab is this window's: keep the tab
      // this window had selected in whichever column now holds it, else the
      // column's first tab.
      const liveColumnSelections = new Set<string>()
      for (const g of get().extraGroups) if (g.activeTabId) liveColumnSelections.add(g.activeTabId)
      const restoredExtraGroups: Array<{ tabs: Tab[], activeTabId: string | null }> = []
      for (const group of layout.extraGroups ?? []) {
        const groupTabs = (Array.isArray(group?.tabs) ? group.tabs : []).map(restoreTab)
        const kept = groupTabs.find((t) => liveColumnSelections.has(t.id))
        restoredExtraGroups.push({
          tabs: groupTabs,
          activeTabId: kept ? kept.id : (groupTabs[0]?.id ?? null),
        })
      }
      const restoredSplitCount = Math.min(
        3,
        Math.max(1, layout.splitCount ?? 1, 1 + restoredExtraGroups.length),
      )
      while (restoredExtraGroups.length < restoredSplitCount - 1) {
        restoredExtraGroups.push({ tabs: [], activeTabId: null })
      }

      // T11 — restamp the built array BEFORE set(). Never await chat/list here.
      const restampedTabs = restampBuiltTabs(restoredTabs, liveById)
      const restampedExtraGroups = restoredExtraGroups.map((g) => ({
        ...g,
        tabs: restampBuiltTabs(g.tabs, liveById),
      }))
      const restampedActiveTabId = restampedTabs.some((t) => t.id === restoredActiveTabId)
        ? restoredActiveTabId
        : (restampedTabs[0]?.id ?? null)

      set({
        tabs: restampedTabs,
        activeTabId: restampedActiveTabId,
        extraGroups: restampedExtraGroups,
        splitCount: restoredSplitCount,
        // D2 — the focused column is this window's, never the saved layout's.
        // A remote adoption keeps it (clamped); a fresh load starts at 0
        // because `clearAllTabs` reset it.
        activeGroupIndex: Math.min(get().activeGroupIndex, restoredSplitCount - 1),
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

    saveLayoutForWorkspace: (projectId: string, workspaceId: string, opts?: { force?: boolean }) => {
      // 0.39.39 (#676/#677) — silent remote-reorder adoption: never echo a save
      // back while applying a peer's layout (would re-broadcast and ping-pong).
      if (isLayoutSaveSuppressed()) return
      const key = `${projectId}:${workspaceId}`
      // The layout serialized is this window's CURRENT state; it only belongs
      // to `key` while `key` is the open workspace.
      if (get().activeWorkspaceKey !== key) return
      // V16 — an empty strip saves `{"version":2,"tabs":[]}` instead of
      // deleting the row, so the revision keeps climbing and other windows
      // hear about it. Every reader treats empty `tabs` as "no layout" (next
      // open shows the empty-workspace hints / default agent).
      //
      // V17 — goes through the based save (`baseRevision`, merge on 409). The
      // monotonic `revision` (#677.3) rides back and becomes our base.
      void submitLayoutSave(key, activeLayoutSaveJob(key, projectId, workspaceId, {
        allowEmpty: true,
        force: opts?.force,
        label: 'Failed to persist workspace layout',
      }))
    },

    loadLayoutForWorkspace: async (projectId: string, workspaceId: string, cwd: string): Promise<void> => {
      assertRoomWorkspace(projectId, workspaceId)
      const key = `${projectId}:${workspaceId}`
      // per-client-view-state.md (Phase 2) — thread the workspace ids into the
      // store BEFORE `restoreLayout` runs (it reads them via
      // `resolveRestoredSelection` to source this client's selection) and so
      // later group-0 selection writes (`setActiveTabInGroup`) have the key.
      set({ activeProjectId: projectId, activeWorkspaceId: workspaceId })
      const restampNamedChats = () => {
        const st = get()
        void restampSessionTabsFromChatList(scope, cwd, collectStoreTabs(st), st.setTabTitle)
      }
      const savedLayout = get().workspaceLayouts[key]
      layoutCwds.set(key, cwd)
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
        // Surfaced = every pane group across group 0 AND the split columns
        // (same rule as `isPaneGroupSurfaced`). Scanning only `state.tabs`
        // here re-adopted live split-column terminals into the MAIN group.
        const surfacedPgIds = new Set<string>()
        for (const tab of state.tabs) {
          tab.paneGroups.forEach((_, pgId) => surfacedPgIds.add(pgId))
        }
        for (const group of state.extraGroups) {
          for (const tab of group.tabs) {
            tab.paneGroups.forEach((_, pgId) => surfacedPgIds.add(pgId))
          }
        }

        // 0.38.0 v2 — refresh existing terminal items with daemon-owned
        // command/args/cwd. After v1→v2 migration, the renderer no longer
        // carries this data in layout JSON; the daemon's list endpoint is
        // the canonical source. Mutates TerminalItemData in place so live
        // TerminalPane components pick up the new values via the
        // subscription that fires from the `set` call below.
        // Group 0 AND extraGroups (split-column extras never got this before).
        const refreshTabFromDaemon = (tab: Tab): number => {
          let n = 0
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
              const nextConversation = pickConversationId(
                d.conversationId,
                session.conversationId,
                session.sessionId,
              )
              if (
                d.cwd !== nextCwd ||
                d.command !== nextCmd ||
                !arraysEqual(d.args, nextArgs) ||
                d.sessionId !== nextSession ||
                d.conversationId !== nextConversation
              ) {
                d.cwd = nextCwd
                d.command = nextCmd
                d.args = nextArgs
                d.sessionId = nextSession
                d.conversationId = nextConversation
                n += 1
              }
            }
          }
          return n
        }
        let refreshedItems = 0
        for (const tab of state.tabs) refreshedItems += refreshTabFromDaemon(tab)
        for (const group of state.extraGroups) {
          for (const tab of group.tabs) refreshedItems += refreshTabFromDaemon(tab)
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
              conversationId: pickConversationId(
                undefined,
                session.conversationId,
                session.sessionId,
              ),
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
          set({
            tabs: [...state.tabs, ...adopted],
            extraGroups: state.extraGroups.map((g) => ({ ...g, tabs: [...g.tabs] })),
          })
          // Only persist when adopt changed the on-disk tab set. Refresh-only
          // updates fill in transient fields (cwd/command/args/sessionId)
          // that v2 serialization intentionally drops, so the rewrite would
          // be byte-identical to disk — saving it every restore is a
          // wasted SQL write and adds log noise.
          if (adopted.length > 0) {
            get().saveLayoutForWorkspace(projectId, workspaceId)
          }
        }

        // N5 — conversationId may have just been hydrated from the daemon
        // row; restamp extras from custom_name once.
        restampNamedChats()

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
        // V14/V17 — the cached copy's own revision is the base, never a newer
        // save's: a stale cache then gets a 409 and merges instead of
        // overwriting. Unknown (older daemon) → no base (last-write-wins).
        setLayoutRevisionExact(key, cacheRevisions.get(key))
        setAckedLayout(key, savedLayout)
        get().restoreLayout(savedLayout, cwd)
        set({ activeWorkspaceKey: key })
        healAndSave()
        if (wasPreV2) {
          // v1 → v2 conversion happened inside restoreLayout. Persist the
          // migrated shape so the disk copy converges and the next open
          // is a pure-v2 path.
          console.warn(`[tabs] migrated workspace_layouts to v2 for ${key}`)
          get().saveLayoutForWorkspace(projectId, workspaceId, { force: true })
        }
        restampNamedChats()
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
          // reads `project_id`/`workspace_id`). V14 — `with_revision=1` returns
          // `{layoutJson, revision}` so this window has a base for its first
          // save; an older daemon answers with the bare JSON string.
          const fetched = await fetchLayoutWithRevision(projectId, workspaceId)
          // Guard: bail if user already switched to a different workspace
          if (get().activeWorkspaceKey !== key) return
          setLayoutRevisionExact(key, fetched.revision)
          if (fetched.revision !== undefined) cacheRevisions.set(key, fetched.revision)
          else cacheRevisions.delete(key)
          if (fetched.layout) setAckedLayout(key, fetched.layout)
          else ackedLayouts.delete(key)

          if (fetched.layout) {
            try {
              const layout = fetched.layout
              if (layout.tabs && layout.tabs.length > 0) {
                const wasPreV2 = (layout.version ?? 1) < LAYOUT_SCHEMA_VERSION
                set({ workspaceLayouts: { ...get().workspaceLayouts, [key]: layout } })
                get().restoreLayout(layout, cwd)
                healAndSave()
                if (wasPreV2) {
                  console.warn(`[tabs] migrated workspace_layouts to v2 for ${key}`)
                  get().saveLayoutForWorkspace(projectId, workspaceId, { force: true })
                }
                restampNamedChats()
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
      // `load-all` caches every workspace's layout on the window's server.
      // A pinned room holds one workspace and never bulk-loads.
      assertPrimary('loadWorkspaceSessionsFromDb')
      // Host-staleness guard (#625 corollary): a load started against one
      // host must not land after a switch — the old host's layouts (keyed by
      // ITS project/workspace ids) would repopulate the just-cleared cache,
      // and flipping `hasLoadedWorkspaceSessions` would suppress the retry
      // the NEW host's baseline still needs.
      const loadConnectionKey = scope.connectionKey
      try {
        // The daemon's `workspace-layouts/load-all` route serializes
        // `db_ops::WorkspaceLayout` with `#[serde(rename_all = "camelCase")]`,
        // emitting exactly `{ projectId, workspaceId, layoutJson }`. The old
        // Tauri `workspace_layout_load_all` did an `Into::into` into a
        // field-identical wrapper struct (also camelCase) — a pure rename
        // with no shape change — so the raw daemon response already matches
        // this type and needs NO post-fetch transform.
        const sessions = await daemonCliGet<Array<{ projectId: string, workspaceId: string, layoutJson: string, revision?: number }>>(scope, 'workspace-layouts/load-all')
        if (scope.connectionKey !== loadConnectionKey) return
        const layouts: Record<string, SerializedLayout> = {}
        for (const session of sessions) {
          const rowKey = `${session.projectId}:${session.workspaceId}`
          try {
            layouts[rowKey] = JSON.parse(session.layoutJson)
            // V14 — each row's revision is the base for the cached restore.
            if (typeof session.revision === 'number') cacheRevisions.set(rowKey, session.revision)
            else cacheRevisions.delete(rowKey)
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
      rememberLiveNamedChatTitles(scope, collectStoreTabs(get()))
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
                const r = await daemonCliGet<{ sessionId: string | null }>(scope, 'chat/detect-active', {
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
          // Host-aware (0.40.38) — same fix as fetchDaemonSessions: the
          // Tauri invoke only ever asked the LOCAL daemon.
          const sessions = await daemonCliGet<
            Array<{
              sessionId: string
              agentName: string
              command: string | null
              args: string[]
              cwd: string
              isV2: boolean
            }>
          >(scope, 'sessions/list-for-workspace', { path: cwd })

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

          // Look up default agent preset via the one resolution seam
          // (id-first, legacy-token tolerant, first-enabled fallback), with
          // the workspace's own default (projects.default_agent) taking
          // precedence over the global setting. `key` is
          // `${projectId}:${workspaceId}`.
          let agentOpts: { command?: string; args?: string[]; title?: string } = {}
          try {
            const presetsRef = deps.presets()
            if (presetsRef) {
              const resolved = resolveAgentCommand(
                presetsRef.presets as AgentPresetLike[],
                useSettingsStore.getState().defaultAgent,
                projectDefaultAgentFor(key.split(':')[0] || null),
              )
              if (resolved) {
                agentOpts = {
                  command: resolved.command,
                  args: resolved.args,
                  title: resolved.preset.label,
                }
              }
            }
          } catch { /* fall back to plain terminal */ }

          // Resume previous session if this is a resumable CLI tool (e.g. Claude)
          if (agentOpts.command) {
            const toolConfig = RESUMABLE_CLI_TOOLS[agentOpts.command]
            if (toolConfig) {
              try {
                const r = await daemonCliGet<{ sessionId: string | null }>(scope, 'chat/detect-active', {
                  provider: toolConfig.provider,
                  project_path: cwd,
                })
                const sessionId = r.sessionId
                if (sessionId) {
                  if (toolConfig.resumeSubcommand) {
                    const baseArgs = agentOpts.args ?? []
                    agentOpts.args = [...baseArgs, toolConfig.resumeSubcommand, sessionId]
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
      assertPrimary('stashWorkspace')
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
        // V16 — save the empty layout (not a delete) so loadLayoutForWorkspace
        // falls through to launchDefaultAgent AND the revision keeps climbing.
        // The workspace is leaving this window, so a 409 (it changed elsewhere)
        // drops this write — the other window's layout is newer.
        const [projectId, workspaceId] = key.split(':')
        cacheRevisions.delete(key)
        if (projectId && workspaceId) {
          void submitLayoutSave(key, {
            projectId,
            workspaceId,
            conflict: 'drop',
            label: 'empty-workspace save',
            produce: () => EMPTY_SERIALIZED_LAYOUT,
          })
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
      if (!isPrimary) {
        const [pid, wid] = key.split(':')
        assertRoomWorkspace(pid ?? '', wid ?? '')
      }
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
        layoutCwds.set(key, cwd)
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

      // Serialize + save current active workspace (based save, merge on 409).
      if (state.tabs.length > 0 || state.extraGroups.length > 0) {
        const [projectId, workspaceId] = activeKey.split(':')
        if (projectId && workspaceId && state.activeWorkspaceKey === activeKey) {
          await submitLayoutSave(activeKey, activeLayoutSaveJob(activeKey, projectId, workspaceId, {
            label: 'Failed to save active workspace',
          }))
        }
      }

      // Serialize + save each background workspace. V17 — a 409 drops the
      // write: the window that has that workspace open is newer.
      for (const [key, snapshot] of Object.entries(state.backgroundWorkspaces)) {
        const [projectId, workspaceId] = key.split(':')
        if (projectId && workspaceId) {
          await submitLayoutSave(key, {
            projectId,
            workspaceId,
            conflict: 'drop',
            label: `Failed to save background workspace ${key}`,
            produce: () => serializeSnapshot(snapshot),
          })
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
        persistDebounceTimer = null
        saveActiveWorkspaceLayoutNow()
      }, 1000)
    },

    flushLayoutPersist: (opts?: { allowEmpty?: boolean }) => {
      // 0.40.48: structural mutations that MUST hold (column split/unsplit,
      // and since V22 a tab close) save immediately instead of riding the 1s
      // debounce — closes the window where a competing remote
      // `TabOrderChanged` rebuilds from the pre-mutation layout.
      if (persistDebounceTimer) {
        clearTimeout(persistDebounceTimer)
        persistDebounceTimer = null
      }
      void saveActiveWorkspaceLayoutNow(opts)
    },

  }))

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
      // Host-aware (0.40.38): the old `k2so_sessions_list_for_workspace`
      // Tauri invoke is hard-wired to the LOCAL daemon, so on a remote
      // host reconcile asked the wrong machine, got nothing, and never
      // refilled item `command` — which is what drives tab agent icons
      // (the icons-vanish-on-relogin bug).
      return await daemonCliGet<DaemonSessionRow[]>(scope, 'sessions/list-for-workspace', {
        path: projectPath,
      })
    } catch (err) {
      console.warn('[tabs] daemon list_sessions failed for', projectPath, err)
      return null
    }
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
    conversationId?: string
    /** P3c (D2) — override the agent_name TerminalPane uses on v2/spawn so it
     *  ATTACHES to an existing daemon session (find-or-spawn returns reused:true)
     *  instead of minting a fresh `tab-<paneGroupId>` PTY. Set for API-spawned
     *  sandbox cells whose session is keyed under a host-minted `api-<...>` name,
     *  not `tab-<...>`. Mirrors the heartbeat-surfaced attach mechanism. */
    attachAgentName?: string
    /** P3c (D2) — ask the daemon to (re)resolve a sandbox backend on the v2/spawn
     *  attach, so the spawn response echoes the backend name (belt-and-suspenders
     *  with the SessionAdded `sandbox_backend` field). */
    sandbox?: boolean
    /** P3c (D2) — the resolved sandbox backend name from the SessionAdded event,
     *  stamped immediately so TabBar.tsx lights the D9 orange marker on adoption
     *  rather than waiting for the spawn-response echo. */
    sandboxBackend?: string
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
    if (pg.items[0]?.type === 'terminal') {
      const d = pg.items[0].data as TerminalItemData
      if (args.sessionId) d.sessionId = args.sessionId
      if (args.conversationId && args.conversationId !== args.sessionId) {
        d.conversationId = args.conversationId
      }
    }
    // P3c (D2) — thread the attach/sandbox fields onto the TerminalItemData so
    // TerminalPane attaches to the existing cell (attachAgentName) + the orange
    // marker lights immediately (sandboxBackend). Only stamped when provided, so
    // the existing `tab-`-prefixed adoption path is byte-identical.
    if (pg.items[0]?.type === 'terminal') {
      const d = pg.items[0].data as TerminalItemData
      if (args.attachAgentName) d.attachAgentName = args.attachAgentName
      if (args.sandbox) d.sandbox = true
      if (args.sandboxBackend) d.sandboxBackend = args.sandboxBackend
      if (args.attachAgentName?.startsWith('api-')) d.fromApi = true
    }
    tabCounter++
    return {
      // V22 — a stable id, so two windows adopting the same daemon session
      // make the same tab and the layout merge collapses them.
      id: `adopted-${args.paneGroupId}`,
      title: args.command ?? `Terminal ${tabCounter}`,
      mosaicTree: args.paneGroupId,
      paneGroups: new Map([[args.paneGroupId, pg]]),
    }
  }

  /** Same strip mutation as the workspace `session_removed` handler,
   *  including the layout save. Does not POST `sessions/v2/close`.
   *  `save` is null only when this window has no active workspace ids;
   *  the filter still runs. */
  function dropSurfacedTabsForSessionRemoval(
    pgId: string,
    save: { projectId: string; workspaceId: string } | null,
  ): boolean {
    const state = store.getState()
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
    if (mainDelta === 0 && extraDelta === 0) return false

    let newActiveId = state.activeTabId
    if (newActiveId && !droppedFromMain.find((t) => t.id === newActiveId)) {
      const stillExists = newExtraGroups.some((g) =>
        g.tabs.find((t) => t.id === newActiveId),
      )
      if (!stillExists) {
        newActiveId = droppedFromMain[0]?.id ?? null
      }
    }
    const key = save
      ? `${save.projectId}:${save.workspaceId}`
      : state.activeWorkspaceKey
    console.warn(`[tabs] session_removed push — dropped paneGroup=${pgId} for ${key}`)
    store.setState({
      tabs: droppedFromMain,
      activeTabId: newActiveId,
      extraGroups: newExtraGroups.map((g) => ({
        tabs: g.tabs,
        activeTabId: g.tabs.find((t) => t.id === g.activeTabId)
          ? g.activeTabId
          : g.tabs[0]?.id ?? null,
      })),
    })
    if (save) {
      store.getState().saveLayoutForWorkspace(save.projectId, save.workspaceId)
    }
    return true
  }

  function dropPaneOwnedElsewhere(paneId: string): void {
    const state = store.getState()
    const save =
      state.activeProjectId && state.activeWorkspaceId
        ? { projectId: state.activeProjectId, workspaceId: state.activeWorkspaceId }
        : null
    if (dropSurfacedTabsForSessionRemoval(paneId, save)) return

    const main = stripSplitPane(state.tabs, paneId)
    let extraChanged = false
    const extraGroups = state.extraGroups.map((g) => {
      const stripped = stripSplitPane(g.tabs, paneId)
      if (stripped.changed) extraChanged = true
      return { ...g, tabs: stripped.tabs }
    })
    if (!main.changed && !extraChanged) return
    store.setState({ tabs: main.tabs, extraGroups })
    if (save) {
      store.getState().saveLayoutForWorkspace(save.projectId, save.workspaceId)
    }
  }

  /** Local strip drop after a sidecar refresh spawn failed and this window
   *  already skipped that pane group's SessionRemoved. Same mutation as
   *  `onRemoved`. Does not POST close. */
  function dropTabAfterFailedSidecarRefresh(paneGroupId: string): void {
    const { activeProjectId, activeWorkspaceId } = store.getState()
    const save =
      activeProjectId && activeWorkspaceId
        ? { projectId: activeProjectId, workspaceId: activeWorkspaceId }
        : null
    dropSurfacedTabsForSessionRemoval(paneGroupId, save)
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
    deps.activateProject(projectId)

    tearDownActiveWorkspaceSubscription()
    activeSessionEventsKey = key

    // 0.39.39 (#676/#677) — open the workspace-scoped tab-title / tab-order
    // broadcast subscription alongside the session-events one (capability-
    // gated; against an older/remote daemon these events never arrive and the
    // renderer keeps its local-layout behavior). On (re)connect re-snapshot
    // the canonical tab titles so a rename missed during a drop is backfilled.
    // Home M4 (MS46): a pinned room carries these on its ONE workspace
    // socket (below) instead of a second tab-events socket.
    const tabEvents = scope.serverSupports('daemon-broadcasts')
    const tabHandlers = {
        onTabTitleChanged: (event: TabTitleChangedEvent) => {
          // Match on project_id (the event's `project` is the project PATH
          // echoed, but the title store is keyed by tab id which is globally
          // unique within a workspace — apply to the surfaced tab directly).
          if (store.getState().activeWorkspaceKey !== key) return
          store.getState().applyDaemonTabTitle(event.tabId, event.title, event.locked)
        },
        onTabOrderChanged: (event: TabOrderChangedEvent) => {
          if (store.getState().activeWorkspaceKey !== key) return
          // Only the active workspace's row matters here.
          if (event.project !== projectId || event.workspace !== workspaceId) return
          const base = layoutRevisions.get(key) ?? 0
          if (event.revision <= base) {
            // Our own write (or an older one) — already reflected locally.
            // Still advance the base so a duplicate broadcast is a no-op.
            recordLayoutRevision(key, event.revision)
            return
          }
          // V22 — a refetch is coming: hold `session_added` adoption until it
          // lands, so a peer's new split terminal is placed by the layout (in
          // the peer's column), not adopted into this window's column 0.
          refetchInFlight.add(key)
          void (async () => {
            try {
              // The daemon emits this broadcast BEFORE writing the save
              // response, so OUR OWN in-flight save's echo can outrun the
              // response that advances `layoutRevisions`. Settle in-flight
              // saves first, then re-judge: a self-echo dissolves against
              // the recorded base (refetching here applied a layout that
              // was STALE relative to post-save local changes — it wiped a
              // just-created split column). Only a revision still ahead of
              // the settled base is a genuine remote write.
              await waitLayoutIdle(key)
              if (store.getState().activeWorkspaceKey !== key) return
              const settledBase = layoutRevisions.get(key) ?? 0
              if (event.revision <= settledBase) return
              // A remote client changed the layout ahead of our base — re-fetch
              // the canonical layout and adopt it, merging any change of ours
              // the daemon has not confirmed. The base advances only once the
              // fetched layout is applied (never before: a save sent in between
              // would carry a base whose content this window never saw).
              await refetchLayoutForRemoteReorder(key, projectId, workspaceId, cwd, event.revision)
            } finally {
              refetchInFlight.delete(key)
              drainDeferredAdoptions(key)
            }
          })()
        },
    }
    if (tabEvents) {
      void applyTabTitlesSnapshot(projectId)
      if (isPrimary) {
        activeTabEventsUnsub = subscribeToWorkspaceTabEvents(scope, cwd, {
          ...tabHandlers,
          onHello: () => {
            if (store.getState().activeWorkspaceKey !== key) return
            void applyTabTitlesSnapshot(projectId)
          },
        })
      }
    }

    activeSessionEventsUnsub = subscribeToWorkspaceSessionEvents(scope, cwd, {
      ...(tabEvents && !isPrimary ? tabHandlers : {}),
      // Home M4: a pinned room's socket feeds its server's app bus
      // (presence, Active set) while that server has no app socket here.
      carryAppBus: !isPrimary,
      onAdded: (event: SessionAddedEvent) => {
        // Only adopt `tab-<paneGroupId>` sessions — pinned chat and
        // heartbeats live under their own canonical agent_names with
        // their own dedicated surfaces. Forwarded events still drive
        // the mobile companion, the renderer just ignores them.
        const pgId = event.pane_group_id
        if (!pgId || !event.agent_name.startsWith('tab-')) return
        // Workspace switched while the event was in flight — bail.
        if (store.getState().activeWorkspaceKey !== key) return
        const adopt = (): void => {
          if (store.getState().activeWorkspaceKey !== key) return
          const state = store.getState()
          if (isPaneGroupSurfaced(state, pgId)) return
          const tab = buildAdoptedTerminalTab({
            paneGroupId: pgId,
            cwd: event.workspace_path || cwd,
            command: event.command ?? undefined,
            args: event.args.length > 0 ? event.args : undefined,
            sessionId: event.session_id,
          })
          console.warn(`[tabs] session_added push — adopting paneGroup=${pgId} for ${key}`)
          store.setState((s) => ({ tabs: [...s.tabs, tab] }))
          // V22 — through the based save (merge on 409), not a blind write.
          store.getState().saveLayoutForWorkspace(projectId, workspaceId)
        }
        // V22 — while a remote layout refetch is pending, adopt only after it
        // lands (the refetched layout may already place this pane).
        if (refetchInFlight.has(key)) {
          deferAdoption(key, adopt)
          return
        }
        adopt()
      },
      onRemoved: (event: SessionRemovedEvent) => {
        const pgId = event.pane_group_id
        if (!pgId || !event.agent_name.startsWith('tab-')) return
        // Refresh's own SessionRemoved must not drop the strip tab. A
        // remove with no mark for this pane group still falls through.
        if (takeSessionRemoved(pgId) === 'skip') return
        if (store.getState().activeWorkspaceKey !== key) return
        dropSurfacedTabsForSessionRemoval(pgId, { projectId, workspaceId })
      },
      onHello: () => {
        // First message after (re)connect. No-op on initial connect
        // because the renderer was just reconciled. On a reconnect
        // after a transient drop the renderer could have missed an
        // emit window — defensively re-fetch the daemon's snapshot
        // and re-run the orphan-adoption pass.
        if (store.getState().activeWorkspaceKey !== key) return
        // Pinned room: this one socket also carries the tab titles.
        if (tabEvents && !isPrimary) void applyTabTitlesSnapshot(projectId)
        void (async () => {
          const sessions = await fetchDaemonSessions(cwd)
          if (sessions === null) return
          if (store.getState().activeWorkspaceKey !== key) return
          // V22 — same rule as `onAdded`: a pending layout refetch may place
          // these sessions in a peer's column; adopt only after it lands.
          if (refetchInFlight.has(key)) await new Promise<void>((resolve) => deferAdoption(key, resolve))
          if (store.getState().activeWorkspaceKey !== key) return
          const state = store.getState()
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
                conversationId: pickConversationId(undefined, s.conversationId, s.sessionId),
              }),
            )
          }
          if (adopted.length > 0) {
            console.warn(`[tabs] hello reconcile — adopted ${adopted.length} orphan(s) for ${key}`)
            store.setState((s) => ({ tabs: [...s.tabs, ...adopted] }))
            store.getState().saveLayoutForWorkspace(projectId, workspaceId)
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

  /**
   * Place an API-adopted tab on the correct workspace strip.
   * - Active project (path match) → append to in-view `tabs` (no active-tab steal).
   * - Background snapshot for that layout key → append there.
   * - Else merge into `workspaceLayouts` + persist so next open of that
   *   workspace restores the audit tab (Scout sales pilot: never Julie strip).
   */
  function placeApiAdoptedTab(tab: Tab, eventPath: string): void {
    const project = findProjectForPathIn(deps.projectsPathIndex(), eventPath)
    const state = store.getState()

    // Sandbox cells use ephemeral cwds outside any project — keep prior
    // behavior: surface on the focused strip so the operator still sees them.
    if (!project || !project.primaryWorkspaceId) {
      console.warn(
        `[tabs] api-session adoption — no registered project for path=${eventPath || '(empty)'}; surfacing on active strip`,
      )
      store.setState((s) => ({ tabs: [...s.tabs, tab] }))
      return
    }

    const layoutKey = `${project.id}:${project.primaryWorkspaceId}`
    const activeKey = state.activeWorkspaceKey
    const onActiveProject =
      state.activeProjectId === project.id ||
      (activeKey != null && activeKey.startsWith(`${project.id}:`))

    if (onActiveProject) {
      console.warn(
        `[tabs] api-session adoption — path=${eventPath} → active project ${project.id}; appending to focused strip`,
      )
      store.setState((s) => ({ tabs: [...s.tabs, tab] }))
      return
    }

    // Non-active: park without yanking focus.
    const bg = state.backgroundWorkspaces[layoutKey]
    if (bg) {
      console.warn(
        `[tabs] api-session adoption — path=${eventPath} → background key=${layoutKey}`,
      )
      store.setState((s) => {
        const cur = s.backgroundWorkspaces[layoutKey]
        if (!cur) return s
        const nextSnap: WorkspaceTabSnapshot = {
          ...cur,
          tabs: [...cur.tabs, tab],
        }
        return {
          backgroundWorkspaces: {
            ...s.backgroundWorkspaces,
            [layoutKey]: nextSnap,
          },
          workspaceLayouts: {
            ...s.workspaceLayouts,
            [layoutKey]: serializeSnapshot(nextSnap),
          },
        }
      })
      const layout = store.getState().workspaceLayouts[layoutKey]
      if (layout) {
        // V17 — based save; a 409 drops it (the workspace is open, and newer,
        // somewhere else; that window adopts the session itself).
        void submitLayoutSave(layoutKey, {
          projectId: project.id,
          workspaceId: project.primaryWorkspaceId,
          conflict: 'drop',
          label: 'api-session park save (background)',
          produce: () => layout,
        })
      }
      return
    }

    // Never opened this session: merge into cached/DB layout only.
    console.warn(
      `[tabs] api-session adoption — path=${eventPath} → layout key=${layoutKey} (not active)`,
    )
    const existing = state.workspaceLayouts[layoutKey]
    const alone = serializeSnapshot({
      tabs: [tab],
      extraGroups: [],
      splitCount: 1,
      activeGroupIndex: 0,
      activeTabId: null,
    })
    const nextLayout: SerializedLayout = existing?.tabs?.length
      ? {
          ...existing,
          tabs: [...existing.tabs, ...(alone.tabs ?? [])],
        }
      : alone
    store.setState((s) => ({
      workspaceLayouts: {
        ...s.workspaceLayouts,
        [layoutKey]: nextLayout,
      },
    }))
    // V17 — based save; a 409 drops it and forgets the stale cached copy.
    void submitLayoutSave(layoutKey, {
      projectId: project.id,
      workspaceId: project.primaryWorkspaceId,
      conflict: 'drop',
      label: 'api-session park save (layout)',
      produce: () => nextLayout,
    })
  }

  /**
   * Plus-menu Option-click. Places the orange microvm tab in `groupIndex` and
   * focuses it there. A SessionAdded that already parked the cell on the primary
   * strip is moved. `hideApiSessions` does not apply — this is the click the
   * user just made. Broadcast adoption stays on `adoptApiSandboxSession`.
   */
  function placeClickedSandboxTab(args: {
    groupIndex: number
    cwd: string
    sessionId: string
    agentName: string
  }): void {
    const found = locateClickedSandboxTab(args.agentName, args.sessionId)
    if (found) {
      const state = store.getState()
      const destOk =
        args.groupIndex === 0 || args.groupIndex - 1 < state.extraGroups.length
      if (found.group !== args.groupIndex && destOk) {
        state.moveTabToGroup(found.group, args.groupIndex, found.tabId)
      }
      store.getState().setActiveTabInGroup(args.groupIndex, found.tabId)
      return
    }

    const paneGroupId = crypto.randomUUID()
    const tab = buildAdoptedTerminalTab({
      paneGroupId,
      cwd: args.cwd,
      sessionId: args.sessionId,
      attachAgentName: args.agentName,
      sandbox: true,
      sandboxBackend: 'microvm',
    })
    const firstItem = [...tab.paneGroups.values()][0]?.items[0]
    if (firstItem?.type === 'terminal') {
      (firstItem.data as TerminalItemData).renderer = 'kessel'
    }
    const state = store.getState()
    if (args.groupIndex <= 0) {
      store.setState({ tabs: [...state.tabs, tab] })
    } else {
      const gi = args.groupIndex - 1
      if (gi < 0 || gi >= state.extraGroups.length) return
      const groups = state.extraGroups.slice()
      groups[gi] = { ...groups[gi], tabs: [...groups[gi].tabs, tab] }
      store.setState({ extraGroups: groups })
    }
    store.getState().setActiveTabInGroup(args.groupIndex, tab.id)
  }

  function locateClickedSandboxTab(
    agentName: string,
    sessionId: string,
  ): { group: number; tabId: string } | null {
    const state = store.getState()
    for (const tab of state.tabs) {
      if (tabMatchesApiSession(tab, agentName, sessionId)) {
        return { group: 0, tabId: tab.id }
      }
    }
    for (let i = 0; i < state.extraGroups.length; i++) {
      for (const tab of state.extraGroups[i].tabs) {
        if (tabMatchesApiSession(tab, agentName, sessionId)) {
          return { group: i + 1, tabId: tab.id }
        }
      }
    }
    return null
  }

  function adoptApiSandboxSession(event: SessionAddedEvent): boolean {
    // Scope: only adopt API-labelled cells. Daemon stamps sandbox_backend for
    // real sandboxes (`microvm`) and host-sessions (`host`); bare PTYs omit it.
    const backend = event.sandbox_backend
    if (!backend) return false
    const agentName = event.agent_name
    if (!agentName) return false
    // Workspace "hide sessions": do not auto-surface onto the strip.
    // Explicit Chat-history clicks call openApiHostSessionTab (force).
    if (!event.forceAdopt) {
      const project = findProjectForPathIn(deps.projectsPathIndex(), event.workspace_path || '')
      if (project?.hideApiSessions) return false
    }

    const state = store.getState()
    // De-dupe: the spawning/owning window (or a re-delivered event) already has
    // it — do nothing. Prevents the double-adopt the PRD calls out. Post-reap
    // resume mints a fresh agent_name but reuses the caller's session id; the
    // reaper-close path below must have dropped the zombie tab first, or this
    // sessionId match would swallow the new audit surface.
    if (isApiSandboxSessionSurfaced(state, agentName, event.session_id)) return false

    // Use a fresh local paneGroup/terminal id — the cell is keyed daemon-side by
    // `attachAgentName`, not by a `tab-`-shaped id, so the local id is purely the
    // pane's own identity for the grid WS attach.
    const paneGroupId = crypto.randomUUID()
    // `"host"` is the API label for non-sandboxed host sessions — never ask the
    // daemon to provision a jail on attach. Real backends (e.g. microvm) do.
    const isRealSandbox = backend !== 'host'
    const tab = buildAdoptedTerminalTab({
      paneGroupId,
      cwd: event.workspace_path || '',
      command: event.command ?? undefined,
      args: event.args.length > 0 ? event.args : undefined,
      sessionId: event.session_id,
      attachAgentName: agentName,
      sandbox: isRealSandbox,
      sandboxBackend: backend,
    })
    // Kessel is the daemon-owned renderer; makeTerminalPaneGroup stamps the
    // user's current renderer preference, so force Kessel explicitly — the
    // daemon session it attaches to IS a Kessel (v2) session.
    const firstItem = [...tab.paneGroups.values()][0]?.items[0]
    if (firstItem?.type === 'terminal') {
      (firstItem.data as TerminalItemData).renderer = 'kessel'
    }
    console.warn(
      `[tabs] api-session adoption — agent=${agentName} backend=${backend} path=${event.workspace_path || ''}`,
    )
    // Scout sales pilot: park under the project for event.workspace_path —
    // never append into an unrelated focused workspace (e.g. Julie while
    // sales host-session spawns).
    placeApiAdoptedTab(tab, event.workspace_path || '')
    return true
  }

  /** Explicit Chat-history open: adopt (even when hide-sessions is on) and focus. */
  function openApiHostSessionTab(event: SessionAddedEvent): boolean {
    const forced: SessionAddedEvent = { ...event, forceAdopt: true }
    const state = store.getState()
    const already = isApiSandboxSessionSurfaced(state, event.agent_name, event.session_id)
    if (!already) {
      adoptApiSandboxSession(forced)
    }
    const next = store.getState()
    const find = (tabs: Tab[]): Tab | undefined =>
      tabs.find((t) => tabMatchesApiSession(t, event.agent_name, event.session_id))
    const hit =
      find(next.tabs) ??
      next.extraGroups.flatMap((g) => g.tabs).find((t) => tabMatchesApiSession(t, event.agent_name, event.session_id))
    if (hit) {
      store.getState().setActiveTab(hit.id)
      return true
    }
    return already
  }

  /** Hide-sessions ON: drop API cockpit tabs without killing PTYs. */
  function minimizeApiSessionsForWorkspace(workspacePath: string): number {
    const project = findProjectForPathIn(deps.projectsPathIndex(), workspacePath)
    const state = store.getState()
    const isApiTab = (t: Tab): boolean => {
      for (const pg of t.paneGroups.values()) {
        for (const item of pg.items) {
          if (item.type !== 'terminal') continue
          if (isApiOriginTerminal(item.data as TerminalItemData)) return true
        }
      }
      return false
    }
    const inWorkspace = (t: Tab): boolean => {
      if (!project) return isApiTab(t)
      for (const pg of t.paneGroups.values()) {
        for (const item of pg.items) {
          if (item.type !== 'terminal') continue
          const d = item.data as TerminalItemData
          if (!isApiOriginTerminal(d)) continue
          const cwd = d.cwd || d.projectPath || ''
          if (!cwd) return true
          return findProjectForPathIn(deps.projectsPathIndex(), cwd)?.id === project.id
        }
      }
      return false
    }
    const drop = (tabs: Tab[]): Tab[] => tabs.filter((t) => !inWorkspace(t))
    const main = drop(state.tabs)
    const extra = state.extraGroups.map((g) => ({
      tabs: drop(g.tabs),
      activeTabId: g.activeTabId,
    }))
    let bgDelta = 0
    const nextBg: Record<string, WorkspaceTabSnapshot> = {}
    for (const [key, snap] of Object.entries(state.backgroundWorkspaces)) {
      const tabs = drop(snap.tabs)
      const extraGroups = snap.extraGroups.map((g) => ({
        tabs: drop(g.tabs),
        activeTabId: g.activeTabId,
      }))
      bgDelta +=
        snap.tabs.length -
        tabs.length +
        snap.extraGroups.reduce((n, g, i) => n + (g.tabs.length - extraGroups[i].tabs.length), 0)
      nextBg[key] = { ...snap, tabs, extraGroups }
    }
    const mainDelta = state.tabs.length - main.length
    const extraDelta = state.extraGroups.reduce((n, g, i) => n + (g.tabs.length - extra[i].tabs.length), 0)
    if (mainDelta === 0 && extraDelta === 0 && bgDelta === 0) return 0
    let newActiveId = state.activeTabId
    if (newActiveId && !main.find((t) => t.id === newActiveId)) {
      const still = extra.some((g) => g.tabs.find((t) => t.id === newActiveId))
      if (!still) newActiveId = main[0]?.id ?? null
    }
    store.setState({
      tabs: main,
      activeTabId: newActiveId,
      extraGroups: extra.map((g) => ({
        tabs: g.tabs,
        activeTabId: g.tabs.find((t) => t.id === g.activeTabId) ? g.activeTabId : g.tabs[0]?.id ?? null,
      })),
      backgroundWorkspaces: nextBg,
    })
    return mainDelta + extraDelta + bgDelta
  }

  /** Close cockpit tabs that were surfacing an API-spawned session after the
   *  daemon reaped / unregistered it. Returns true when at least one tab was
   *  dropped. Exported for unit testing.
   *
   *  Does NOT issue v2/close — the PTY is already dead (reaper `kill()` →
   *  ChildExit → unregister). Mirrors the workspace-scoped `onRemoved` path
   *  that only filters tab state. Leaving the zombie tab would block post-reap
   *  resume adoption (de-dupe keys on sessionId) and strand the user on a dead
   *  pane while the revived PTY lives under a new `api-…` agent_name. */
  function dropApiSpawnedSession(event: SessionRemovedEvent): boolean {
    const agentName = event.agent_name
    // Scope: only the host-minted `api-…` namespace (sandbox cells + host
    // sessions). Workspace `tab-` removals stay on the workspace consumer.
    if (!agentName || !agentName.startsWith('api-')) return false

    const state = store.getState()
    const droppedFromMain = state.tabs.filter(
      (t) => !tabIsDropCandidateForApiSessionRemoval(t, agentName),
    )
    const newExtraGroups = state.extraGroups.map((g) => ({
      tabs: g.tabs.filter((t) => !tabIsDropCandidateForApiSessionRemoval(t, agentName)),
      activeTabId: g.activeTabId,
    }))
    const mainDelta = state.tabs.length - droppedFromMain.length
    const extraDelta = state.extraGroups.reduce(
      (n, g, i) => n + (g.tabs.length - newExtraGroups[i].tabs.length),
      0,
    )

    // Also drop from background (parked) strips for non-active workspaces.
    let bgDelta = 0
    const nextBg: Record<string, WorkspaceTabSnapshot> = {}
    for (const [key, snap] of Object.entries(state.backgroundWorkspaces)) {
      const tabs = snap.tabs.filter(
        (t) => !tabIsDropCandidateForApiSessionRemoval(t, agentName),
      )
      const extraGroups = snap.extraGroups.map((g) => ({
        tabs: g.tabs.filter((t) => !tabIsDropCandidateForApiSessionRemoval(t, agentName)),
        activeTabId: g.activeTabId,
      }))
      const d =
        snap.tabs.length -
        tabs.length +
        snap.extraGroups.reduce(
          (n, g, i) => n + (g.tabs.length - extraGroups[i].tabs.length),
          0,
        )
      bgDelta += d
      nextBg[key] = {
        ...snap,
        tabs,
        extraGroups: extraGroups.map((g) => ({
          tabs: g.tabs,
          activeTabId: g.tabs.find((t) => t.id === g.activeTabId)
            ? g.activeTabId
            : g.tabs[0]?.id ?? null,
        })),
      }
    }

    if (mainDelta === 0 && extraDelta === 0 && bgDelta === 0) return false

    let newActiveId = state.activeTabId
    if (newActiveId && !droppedFromMain.find((t) => t.id === newActiveId)) {
      const stillExists = newExtraGroups.some((g) =>
        g.tabs.find((t) => t.id === newActiveId),
      )
      if (!stillExists) {
        newActiveId = droppedFromMain[0]?.id ?? null
      }
    }
    console.warn(
      `[tabs] api-session removed — closed ${mainDelta + extraDelta + bgDelta} audit tab(s) for agent=${agentName}`,
    )
    store.setState({
      tabs: droppedFromMain,
      activeTabId: newActiveId,
      extraGroups: newExtraGroups.map((g) => ({
        tabs: g.tabs,
        activeTabId: g.tabs.find((t) => t.id === g.activeTabId)
          ? g.activeTabId
          : g.tabs[0]?.id ?? null,
      })),
      backgroundWorkspaces: nextBg,
    })
    return true
  }

  /**
   * Hydrate-on-connect (option B / Sales dogfood): late-joining GUIs miss
   * live `SessionAdded` for API host-sessions / sandbox cells spawned while
   * offline. Snapshot live PTYs whose `agentName` is in the host-minted
   * `api-…` namespace and run the same adopt path as a live event.
   *
   * `list-for-workspace?path=/` matches every absolute cwd (same rule as
   * Feedback's live-session probe). Backend is assumed `"host"` when we
   * only have the name prefix — real microvm cells also use `api-…` but
   * attach still reuses the existing PTY via `attachAgentName`; wrong
   * orange label is a polish follow-up (extend list endpoint with backend).
   *
   * Returns how many tabs were newly adopted (0 if all already surfaced
   * or daemon empty/unreachable). Exported for unit tests.
   */
  async function hydrateApiSandboxSessions(): Promise<number> {
    let rows: DaemonSessionRow[] | null = null
    try {
      rows = await daemonCliGet<DaemonSessionRow[]>(scope, 'sessions/list-for-workspace', {
        path: '/',
      })
    } catch (err) {
      console.warn('[tabs] api-session hydrate list failed:', err)
      return 0
    }
    if (!Array.isArray(rows)) return 0

    let adopted = 0
    for (const row of rows) {
      if (!row || !row.isV2) continue
      const agentName = row.agentName
      if (typeof agentName !== 'string' || !agentName.startsWith('api-')) continue
      const sessionId = row.sessionId
      if (typeof sessionId !== 'string' || !sessionId) continue

      const event: SessionAddedEvent = {
        kind: 'session_added',
        workspace_path: typeof row.cwd === 'string' ? row.cwd : '',
        pane_group_id: null,
        agent_name: agentName,
        command: row.command ?? null,
        args: Array.isArray(row.args) ? row.args : [],
        session_id: sessionId,
        isV2: true,
        sandbox_backend: 'host',
      }
      try {
        if (adoptApiSandboxSession(event)) adopted += 1
      } catch (err) {
        console.warn('[tabs] api-session hydrate adopt failed:', agentName, err)
      }
    }
    if (adopted > 0) {
      console.warn(`[tabs] api-session hydrate — adopted ${adopted} live api- session(s)`)
    }
    return adopted
  }

  /** Wire the app-level API-session adoption + reaper-close consumers. Call
   *  ONCE at app boot. Returns an unsubscribe fn. The registries are
   *  module-level and survive host switches, so a single registration covers
   *  the app lifetime; on a host switch the new host's app-level WS feeds the
   *  same registries.
   *
   *  Also hydrates on every app-level `hello` (connect / reconnect / host
   *  switch) so late joiners get tabs for api- sessions already running. */
  function initApiSandboxTabAdoption(): UnsubscribeFn {
    const offAdded = onSessionAddedApp(scope, (event) => {
      try {
        adoptApiSandboxSession(event)
      } catch (err) {
        console.warn('[tabs] api-session adoption failed:', err)
      }
    })
    const offRemoved = onSessionRemovedApp(scope, (event) => {
      try {
        dropApiSpawnedSession(event)
      } catch (err) {
        console.warn('[tabs] api-session drop failed:', err)
      }
    })
    const offHello = onAppHello(scope, () => {
      void hydrateApiSandboxSessions()
    })
    // Immediate pass in case hello already fired before we registered.
    void hydrateApiSandboxSessions()
    return () => {
      offAdded()
      offRemoved()
      offHello()
    }
  }

  /** Browser-pane arc (0.40.34) — wire the app-level `open_url` consumer:
   *  when the daemon routes an http(s) open here (the `k2 open <url>` shim
   *  or a terminal hyperlink click through /cli/fs/open-external), surface
   *  it in a NEW embedded browser tab. Call ONCE at app boot (App.tsx,
   *  alongside `initApiSandboxTabAdoption`). Returns an unsubscribe fn.
   *  The `onOpenUrl` registry is module-level and survives host switches,
   *  so a single registration covers the app lifetime — the local daemon's
   *  and any remote host's app-level WS both feed the same registry. The
   *  daemon has already validated the scheme is http/https; blank/empty
   *  URLs are ignored defensively. One tab per event by design (no dedupe
   *  — repeated opens of the same URL intentionally mint repeated tabs). */
  function initOpenUrlBrowserTabs(): UnsubscribeFn {
    // Hosted web: native Browser pane amputated — ignore open_url surface.
    if (!webFeatures.browserPane) {
      return () => {}
    }
    return onOpenUrl(scope, (url) => {
      const trimmed = typeof url === 'string' ? url.trim() : ''
      if (!trimmed) return
      try {
        store.getState().openUrlInNewTab(trimmed)
      } catch (err) {
        console.warn('[tabs] open_url tab surface failed:', err)
      }
    })
  }

  /** Fetch the daemon-canonical tab titles for a project and apply them to
   *  the surfaced tabs (hydrate labels from the daemon on workspace load +
   *  on reconnect). No-op for tab ids not currently surfaced. Best-effort:
   *  swallows the route-absent / transient case (the local layout label
   *  remains the fallback). */
  async function applyTabTitlesSnapshot(projectId: string): Promise<void> {
    try {
      const titles = await daemonCliGet<DaemonTabTitle[]>(scope, 'workspace/tab-titles', {
        project_id: projectId,
      })
      if (!Array.isArray(titles)) return
      const state = store.getState()
      for (const t of titles) {
        if (t && typeof t.tabId === 'string' && typeof t.title === 'string') {
          rememberTabTitleSnapshot(scope, t.tabId, t.title, t.locked)
          // Carry the daemon's `locked` flag so a sticky user-rename stays
          // locked here (auto PTY/session titles won't overwrite it).
          state.applyDaemonTabTitle(t.tabId, t.title, typeof t.locked === 'boolean' ? t.locked : undefined)
        }
      }
    } catch (err) {
      console.debug('[tabs] tab-titles snapshot skipped:', err)
    }
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
    const savedSig = getSelectedTab(scope, activeProjectId, activeWorkspaceId)
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
    setSelectedTab(scope, activeProjectId, activeWorkspaceId, liveTabSignature(tab))
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
    const state = store.getState()

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
      // T1/T3 — adopt is local (do not call setTabTitle; that POSTs).
      // Omitted st.locked keeps live.locked. Never write locked: undefined.
      const adopted = adoptTabTitle(
        {
          title: live.title,
          locked: live.locked,
          conversationId: conversationIdFromTab(live),
        },
        {
          title: st.title,
          locked: st.locked,
          conversationId: conversationIdFromTab(live),
        },
      )
      if (adopted.title === live.title && live.locked === adopted.locked) {
        reordered.push(live)
      } else {
        reordered.push({ ...live, title: adopted.title, locked: adopted.locked })
      }
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

    store.setState((s) => ({
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
    eventRevision?: number,
  ): Promise<void> {
    // Runs in the layout lane: no save of ours goes out while the refetch is
    // being applied (it would carry a base about to be replaced).
    await runInLayoutLane(key, async () => {
      let fetched: FetchedLayout
      try {
        fetched = await fetchLayoutWithRevision(projectId, workspaceId)
      } catch (err) {
        console.warn('[tabs] remote-reorder re-fetch failed:', err)
        return
      }
      if (store.getState().activeWorkspaceKey !== key) return
      // An older daemon has no revision on read; the broadcast's is the best
      // we know for the layout we just read.
      const revision = fetched.revision ?? eventRevision
      if (typeof revision === 'number' && revision <= (layoutRevisions.get(key) ?? -1)) return
      const layout = fetched.layout
      if (!layout || !layout.tabs || layout.tabs.length === 0) {
        // Empty remote layout: keep this window's tabs (unchanged from before —
        // a view-clear elsewhere must not wipe this window), but take the base
        // so our next save is not refused forever.
        setLayoutRevisionExact(key, revision)
        if (layout) setAckedLayout(key, layout)
        return
      }
      if (hasUnackedLocalLayout(key)) {
        // V18 — this window has a change the daemon has not confirmed (the
        // debounce is armed, or local differs from acked). Merge it onto the
        // newer layout and save the result — the old `cancelPendingLayoutSave()`
        // here threw that change away (a closed tab came back).
        const local = store.getState().serializeCurrentLayout()
        const base = ackedLayouts.get(key)?.layout ?? null
        const merged = mergeSerializedLayouts(base, local, layout)
        setLayoutRevisionExact(key, revision)
        if (typeof revision === 'number') cacheRevisions.set(key, revision)
        setAckedLayout(key, layout)
        cancelPendingLayoutSave()
        applyLayoutSilently(key, merged, cwd)
        console.warn(`[tabs] merged remote layout (revision ${revision ?? '?'}) with this window's change for ${key}`)
        await runLayoutSaveJob(key, activeLayoutSaveJob(key, projectId, workspaceId, {
          label: 'merged layout save',
        }))
        return
      }
      // Adopt the peer's layout SILENTLY. Whether we reorder in place or fall
      // back to `restoreLayout`, the mutation runs under suppression so the
      // autosave subscription (which fires synchronously inside the store's
      // `set(...)`) does NOT echo a `workspace-layouts/save` back to the daemon
      // — the echo that would re-broadcast `TabOrderChanged` and make two clients
      // ping-pong forever (#676/#677 0.39.39 regression).
      //
      // A pure reorder permutes the EXISTING `Tab` objects in place. Any other
      // change restores with the saved tab / pane-group / item ids (V3), so a
      // pane whose tab survived is not remounted. Selection and the focused
      // column stay this window's (per-client-view-state.md Phase 3, D2).
      //
      // Nothing of ours is unconfirmed here, except with no acked layout to
      // merge against (an older daemon): then the remote wins as before, and a
      // pending pre-adoption autosave must not fire ~1s later and echo it.
      cancelPendingLayoutSave()
      setLayoutRevisionExact(key, revision)
      if (typeof revision === 'number') cacheRevisions.set(key, revision)
      setAckedLayout(key, layout)
      applyLayoutSilently(key, layout, cwd)
      console.warn(`[tabs] adopted remote layout (revision ${revision ?? '?'}) for ${key}`)
    })
  }

  // ── Workspace ops (`workspace:*` from the Workspace Assistant) ───────────
  //
  // The Tauri listeners used to live here, wired at import time on THE store,
  // so every room would have acted on every event. One window-level router
  // (`lib/workspace-ops-router.ts`) now listens once and delivers each op to
  // the focused room only (MS18); this applies it to this room's tabs.

  function applyWorkspaceOp(op: WorkspaceOp): void {
    switch (op.kind) {
      // workspace:split-pane -> split an existing pane in a tab
      case 'split-pane': {
        const { tabId, paneId, direction } = op.payload
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
        return
      }

      // workspace:close-pane -> remove a paneGroup from a tab
      case 'close-pane': {
        const { tabId, paneId } = op.payload
        store.getState().removePaneFromTab(tabId, paneId)
        return
      }

      // workspace:open-document -> add a file-viewer item to the paneGroup
      case 'open-document': {
        const { tabId, paneId, filePath } = op.payload
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
        return
      }

      // workspace:open-terminal -> add a terminal item or create a new paneGroup
      case 'open-terminal': {
        const { tabId, paneId, cwd, command } = op.payload
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
              // 2026-07-02 — this was the one terminal-creation path
              // that skipped the renderer stamp (PaneGroupView's dev
              // warning flags exactly this), so its items rendered AND
              // closed on the legacy Alacritty path. Stamp like every
              // other creation path (makeTerminalPaneGroup /
              // paneDataToItem) so render + A6 close both dispatch to
              // the daemon-hosted stack.
              renderer: currentRenderer(),
            },
          }
          state.addItemToPaneGroup(tabId, paneId, newItem)
        } else {
          // PaneGroup doesn't exist — create it and add to mosaic
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
        return
      }

      // workspace:new-tab -> create a new tab
      case 'new-tab': {
        store.getState().addTab(op.payload.cwd)
        return
      }

      // workspace:close-tab -> close a tab
      case 'close-tab': {
        store.getState().removeTab(op.payload.tabId)
        return
      }

      // workspace:arrange -> build a full layout from a descriptor
      case 'arrange': {
        const descriptor = op.payload
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
        return
      }
    }
  }

  // ── Lifecycle wiring ─────────────────────────────────────────────────────

  /** #625 — server-switch reset of the primary room's per-machine state.
   *
   *  `backgroundWorkspaces` / `workspaceLayouts` / `activeWorkspaceKey` are
   *  keyed by the OLD server's project/workspace ids. The store instance
   *  survives the `<App key>` remount on a switch, so without this reset the
   *  IconRail active section + Active Bar rule 4 keep showing the previous
   *  machine's stashed workspaces. Clear the old server's state, drop the
   *  load gate, and reload from the NEW server (the primary scope resolves
   *  the window's server at call time, and `onActiveHostChange` fires after
   *  `activeHost` flips). A pinned room never switches servers. */
  function resetForHostSwitch(): void {
    assertPrimary('resetForHostSwitch')
    hasLoadedWorkspaceSessions = false
    // Cancel debounce + layout-save retry/recovery wait so a queued write
    // keyed to the OLD host cannot fire `daemonCli*` against the NEW host.
    // Never stashWorkspace: an empty stash saves an empty layout on
    // whatever daemon is now active.
    cancelLayoutPersistForHostSwitch()
    // Revisions, acked layouts, and cached revisions belong to the OLD host.
    layoutRevisions.clear()
    ackedLayouts.clear()
    cacheRevisions.clear()
    refetchInFlight.clear()
    deferredAdoptions.clear()
    // Tear down the active workspace's session-events WS to the OLD host so
    // we don't keep a live subscription open against a daemon we've left.
    // The new host's subscription is established when the next workspace is
    // restored (subscribeForActiveWorkspace). Idempotent when no sub is open.
    tearDownActiveWorkspaceSubscription()
    store.setState({
      backgroundWorkspaces: {},
      workspaceLayouts: {},
      activeWorkspaceKey: null,
      activeProjectId: null,
      activeWorkspaceId: null,
      // View-clear — same fields as clearAllTabs. Zustand survives
      // `<App key={hostKey}>`; leftover FileViewer/ImageViewer tabs would
      // GET the previous machine's paths against the new daemon.
      tabs: [],
      extraGroups: [],
      activeTabId: null,
      splitCount: 1,
      activeGroupIndex: 0,
    })
    // per-client-view-state.md (Phase 2) — selection is per-machine; the NEW
    // host has its own workspaces/selections. Clear this machine's stashed
    // selection map so a stale local-host selection can't be matched against a
    // remote-host workspace's tabs. (Mirrors the other per-machine resets.)
    resetSelectedTabs()
    void store.getState().loadWorkspaceSessionsFromDb()
  }

  // Auto-save: subscribe to tab structure changes and persist to the room's
  // server (debounced). Crash resilience — lose at most ~1 second of tab
  // changes. One subscription per instance: a pinned room's tabs only ever
  // save to its own server (MS42).
  //
  // per-client-view-state.md (Phase 1) — `activeTabId` is NOT part of the
  // canonical layout, so a pure SELECTION change is not an autosave trigger:
  // the serialized structure would be byte-identical (a wasted SQL write)
  // AND a save bumps the layout revision + re-broadcasts `TabOrderChanged`
  // to peers — i.e. clicking a tab would needlessly churn every other client.
  // Selection persistence is handled locally by the per-client selected-tabs
  // store (`setActiveTabInGroup`/`setActiveTab`), not this canonical save path.
  const unsubscribeAutosave = store.subscribe((state, prevState) => {
    // Only trigger on meaningful tab structure changes (not backgroundWorkspaces
    // swaps, and not pure selection changes — see note above).
    //
    // D2 — the focused column (`activeGroupIndex`) is per-window view state
    // too: focusing a column is not a save.
    if (
      state.tabs !== prevState.tabs ||
      state.extraGroups !== prevState.extraGroups ||
      state.splitCount !== prevState.splitCount
    ) {
      if (state.activeWorkspaceKey && state.tabs.length > 0) {
        state.persistActiveWorkspace()
      }
    }
  })

  /** Close this room's workspace sockets (the WS would close on its own when
   *  the page unloads; an explicit close gives the daemon a clean disconnect
   *  instead of a TCP RST). */
  function closeSockets(): void {
    activeSessionEventsUnsub?.()
    activeSessionEventsUnsub = null
    activeSessionEventsKey = null
    activeTabEventsUnsub?.()
    activeTabEventsUnsub = null
  }

  let disposed = false
  /** Pinned rooms: flush the based save, then close sockets, timers and the
   *  autosave subscription. The instance is dead afterwards. */
  async function dispose(): Promise<void> {
    assertPinned('dispose')
    if (disposed) return
    disposed = true
    if (persistDebounceTimer) {
      clearTimeout(persistDebounceTimer)
      persistDebounceTimer = null
    }
    unsubscribeAutosave()
    const key = store.getState().activeWorkspaceKey
    if (key) {
      const [projectId, workspaceId] = key.split(':')
      if (projectId && workspaceId) {
        await submitLayoutSave(key, activeLayoutSaveJob(key, projectId, workspaceId, {
          label: 'room close save',
        }))
      }
    }
    cancelLayoutPersistForHostSwitch()
    tearDownActiveWorkspaceSubscription()
  }

  /** Pinned rooms: load the one workspace this room holds. */
  async function open(): Promise<void> {
    assertPinned('open')
    const ws = binding.workspace as TabsRoomWorkspace
    await store.getState().loadLayoutForWorkspace(ws.projectId, ws.workspaceId, ws.path)
  }

  if (isPrimary) {
    // Best-effort cleanup on window unload.
    if (typeof window !== 'undefined') {
      window.addEventListener('beforeunload', closeSockets)
    }

    // Load persisted workspace layouts from the window's server at creation.
    void store.getState().loadWorkspaceSessionsFromDb()

    // Phase 2.5 fix (finding #547): retry the layouts load on daemon
    // (re)connect when the creation-time fetch lost a race with the
    // daemon's startup. Without this, an early-boot failure left
    // `workspaceLayouts` empty for the session, and the next workspace
    // activation built a fresh default layout that auto-save then
    // persisted over the real one.
    onDaemonConnected(() => {
      if (hasLoadedWorkspaceSessions) return
      void store.getState().loadWorkspaceSessionsFromDb()
    })

    onActiveHostChange(() => {
      resetForHostSwitch()
    })
  }

  const room: TabsRoomApi = {
    scope,
    kind: isPrimary ? 'primary' : 'pinned',
    workspace: binding.workspace,
    localCommands: binding.localCommands,
    ensurePinnedAgentTabForMode,
    releasePaneOwnedElsewhere: (paneId) => store.getState().releasePaneOwnedElsewhere(paneId),
    dropTabAfterFailedSidecarRefresh,
    placeClickedSandboxTab,
    adoptApiSandboxSession,
    openApiHostSessionTab,
    minimizeApiSessionsForWorkspace,
    dropApiSpawnedSession,
    hydrateApiSandboxSessions,
    initApiSandboxTabAdoption,
    initOpenUrlBrowserTabs,
    applyWorkspaceOp,
    resetForHostSwitch,
    open,
    dispose,
    __test: {
      layoutRevisions,
      ackedLayouts,
      setAckedLayout,
      isLayoutSaveSuppressed,
      hasLoadedWorkspaceSessions: () => hasLoadedWorkspaceSessions,
      activeSessionEventsKey: () => activeSessionEventsKey,
      tryReorderTabsInPlace,
      resetLayoutSync: () => {
        layoutRevisions.clear()
        ackedLayouts.clear()
        cacheRevisions.clear()
        layoutCwds.clear()
        layoutLanes.clear()
        refetchInFlight.clear()
        deferredAdoptions.clear()
        if (persistDebounceTimer) clearTimeout(persistDebounceTimer)
        persistDebounceTimer = null
      },
    },
  }
  return Object.assign(store, { room })
}

// ── The primary room ─────────────────────────────────────────────────────
//
// The window's own room: today's store. Agents, Home on the connected
// server, and the Focus window all show it. It follows the window's server
// (`primaryScope()` resolves at call time) and the window's selected
// workspace. Code outside a room (the projects store, the sidebar, App's
// quit save) uses it by name; room components get their room's store from
// `RoomProvider` (`stores/room.ts`).
export const useTabsStore: TabsStore = createTabsStore({
  scope: primaryScope(),
  workspace: null,
  deps: primaryDeps,
  localCommands: true,
})

// Module wrappers for the primary room's operations. Window-level and
// primary-only callers (App boot wiring, the projects store, tests) use
// these; room components call their room's `tabs.room.*` instead.

/** Primary room: ensure the pinned agent tabs (projects store). */
export function ensurePinnedAgentTabForMode(agentMode: string, projectPath: string): void {
  useTabsStore.room.ensurePinnedAgentTabForMode(agentMode, projectPath)
}

/** Primary room: spawn 409 `session_owned_elsewhere` drop. */
export function releasePaneOwnedElsewhere(paneId: string): void {
  useTabsStore.room.releasePaneOwnedElsewhere(paneId)
}

/** Primary room: app-boot API-session adoption (App.tsx). */
export function initApiSandboxTabAdoption(): UnsubscribeFn {
  return useTabsStore.room.initApiSandboxTabAdoption()
}

/** Primary room: app-boot `open_url` consumer (App.tsx). */
export function initOpenUrlBrowserTabs(): UnsubscribeFn {
  return useTabsStore.room.initOpenUrlBrowserTabs()
}

/** Primary room: #625 server-switch reset (tests drive it directly). */
export function __resetWorkspaceSessionsForHostSwitch(): void {
  useTabsStore.room.resetForHostSwitch()
}

/** Test-only: the primary room's load gate (#625 reset assertions). */
export function __hasLoadedWorkspaceSessionsForTests(): boolean {
  return useTabsStore.room.__test.hasLoadedWorkspaceSessions()
}

/** Test-only: the primary room's last-known layout revision (0 if unknown). */
export function __getLayoutRevisionForTests(key: string): number {
  return useTabsStore.room.__test.layoutRevisions.get(key) ?? 0
}

/** Test-only: override the primary room's last-known layout revision. */
export function __setLayoutRevisionForTests(key: string, revision: number): void {
  useTabsStore.room.__test.layoutRevisions.set(key, revision)
}

/** Test-only: the primary room's suppression depth (echo-loop regression). */
export function __isLayoutSaveSuppressedForTests(): boolean {
  return useTabsStore.room.__test.isLayoutSaveSuppressed()
}

/** Test-only: forget the primary room's layout-sync bookkeeping (revisions,
 *  acked layouts, lanes, the autosave debounce). Suites that share one store
 *  across tests call this in `beforeEach` — `vi.clearAllTimers` alone leaves
 *  the debounce handle pointing at a dead timer. */
export function __resetLayoutSyncForTests(): void {
  useTabsStore.room.__test.resetLayoutSync()
}

/** Test-only: the primary room's acked (daemon-confirmed) layout. */
export function __getAckedLayoutForTests(key: string): SerializedLayout | undefined {
  return useTabsStore.room.__test.ackedLayouts.get(key)?.layout
}

/** Test-only: seed the primary room's acked layout (as a load would). */
export function __setAckedLayoutForTests(key: string, layout: SerializedLayout): void {
  useTabsStore.room.__test.setAckedLayout(key, layout)
}

/** Test-only: the primary room's open workspace-socket key. */
export function __getActiveSessionEventsKey(): string | null {
  return useTabsStore.room.__test.activeSessionEventsKey()
}

/** Test-only: drive the primary room's in-place reorder adoption (the path
 *  the remote-reorder re-fetch takes for a pure permutation). */
export function __tryReorderTabsInPlaceForTests(key: string, layout: SerializedLayout): boolean {
  return useTabsStore.room.__test.tryReorderTabsInPlace(key, layout)
}

// Primary-room wrappers kept for the store suites (api-sandbox adoption,
// sidecar refresh). Room components never import these: they act on their
// room's `tabs.room` (enforced by stores/room-boundary.test.ts).
export function adoptApiSandboxSession(event: SessionAddedEvent): boolean {
  return useTabsStore.room.adoptApiSandboxSession(event)
}
export function dropApiSpawnedSession(event: SessionRemovedEvent): boolean {
  return useTabsStore.room.dropApiSpawnedSession(event)
}
export function hydrateApiSandboxSessions(): Promise<number> {
  return useTabsStore.room.hydrateApiSandboxSessions()
}
export function placeClickedSandboxTab(args: { groupIndex: number; cwd: string; sessionId: string; agentName: string }): void {
  useTabsStore.room.placeClickedSandboxTab(args)
}
export function openApiHostSessionTab(event: SessionAddedEvent): boolean {
  return useTabsStore.room.openApiHostSessionTab(event)
}
export function minimizeApiSessionsForWorkspace(workspacePath: string): number {
  return useTabsStore.room.minimizeApiSessionsForWorkspace(workspacePath)
}
export function dropTabAfterFailedSidecarRefresh(paneGroupId: string): void {
  useTabsStore.room.dropTabAfterFailedSidecarRefresh(paneGroupId)
}
