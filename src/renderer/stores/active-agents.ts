import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost, daemonCliPostQuery } from '@/lib/daemon-cli'
import { asArray } from '@/lib/as-array'
import {
  terminalCreate,
  terminalExists,
  terminalListRunning,
  type RunningTerminalInfo,
} from '@/lib/terminal-daemon'
import { useTabsStore, type TerminalItemData } from './tabs'
import { useToastStore } from './toast'
import { useProjectsStore } from './projects'
import { usePresetsStore } from './presets'
import { resolveAgentCommand, readProjectDefaultAgent } from '@/lib/agent-resolve'
import { useSettingsStore } from './settings'
import { KNOWN_AGENT_COMMANDS } from '@shared/constants'
import { agentChatId, worktreeChatId } from '@/lib/terminal-id'
// #625 — reset agent pane state on a host switch.
import { onActiveHostChange } from '@/stores/connect-host'
import { serverSupports } from '@/lib/server-capabilities'
import {
  onAppHello,
  onSessionAddedApp,
  onSessionRemovedApp,
  onHooksInstallFailed,
} from '@/stores/session-events'
import { primaryScope } from '@/kessel/server-scope'
import {
  activityStore,
  agentNameForTerminal,
  attachActivity,
  isBusyDisplay,
  registerActivityNotify,
  resetActivity,
  rowForTerminal,
  type ActivityDisplay,
  type ActivityRow,
} from './activity'

// prd-daemon-activity-and-thread-working-v1 S5 (RL9): this store no longer
// decides what an agent is doing. The daemon does (`stores/activity.ts`
// holds its rows); this store keeps the window's live-session bookkeeping
// (`agents` for the quit / close dialogs, `liveSessionCwds` for the Active
// bar's live dot, background spawns) and turns the window server's turn ends
// into toasts.

/** The terminal item (any column, foreground strip) a daemon row is about,
 *  or the pinned Chat tab when the row is a workspace's pinned Chat (its
 *  agent name is its project id, A36). */
function findTabForRow(row: ActivityRow): { tabId: string; groupIndex: number } | null {
  const tabsState = useTabsStore.getState()
  const groups = [tabsState.tabs, ...tabsState.extraGroups.map((g) => g.tabs)]
  for (let gi = 0; gi < groups.length; gi++) {
    for (const tab of groups[gi]) {
      for (const pg of tab.paneGroups.values()) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            if (data.sessionId === row.sessionId || agentNameForTerminal(data) === row.agentName) {
              return { tabId: tab.id, groupIndex: gi }
            }
          }
          if (item.type === 'agent' && row.projectId && row.agentName === row.projectId) {
            const data = item.data as { terminalId?: string; section?: string }
            if (data.section === 'chat' || data.terminalId === agentChatId(row.projectId, '')) {
              return { tabId: tab.id, groupIndex: gi }
            }
          }
        }
      }
    }
  }
  return null
}

/**
 * Best-effort display name for a row's agent, for completion / needs-you
 * toasts: the OWNING workspace's name (the daemon's `projectId`, never the
 * viewed one), then the tab title.
 */
function rowAgentLabel(row: ActivityRow): string | null {
  if (row.projectId) {
    const project = useProjectsStore.getState().projects.find((p) => p.id === row.projectId)
    if (project?.name) return project.name
  }
  const hit = findTabForRow(row)
  if (!hit) return null
  const ts = useTabsStore.getState()
  const tabs = hit.groupIndex === 0 ? ts.tabs : ts.extraGroups[hit.groupIndex - 1]?.tabs ?? []
  return tabs.find((t) => t.id === hit.tabId)?.title || null
}

/**
 * Toast "View" — open the workspace that owns the row (the daemon's
 * `projectId`), then select its tab. Pre-0.40.65 only called setActiveTab on
 * the *current* workspace's tab list, so a finish toast for a background
 * agent never left the foreground workspace.
 */
function navigateToRow(row: ActivityRow): void {
  const select = (): boolean => {
    const hit = findTabForRow(row)
    if (!hit) return false
    if (hit.groupIndex === 0) {
      useTabsStore.getState().setActiveTab(hit.tabId)
    } else {
      useTabsStore.getState().setActiveTabInGroup(hit.groupIndex, hit.tabId)
    }
    return true
  }

  void (async () => {
    const ps = useProjectsStore.getState()
    if (row.projectId && row.projectId !== ps.activeProjectId) {
      const project = ps.projects.find((p) => p.id === row.projectId)
      const ws = project?.workspaces?.[0]
      if (project && ws) {
        // Same gesture as the Active bar row click — stash current tabs,
        // restore this workspace's layout (async).
        ps.setActiveWorkspace(project.id, ws.id)
        for (let i = 0; i < 40; i++) {
          if (findTabForRow(row)) break
          await new Promise((r) => setTimeout(r, 50))
        }
      }
    }
    // Same workspace (or restore finished / never found): select if present.
    select()
  })()
}

/** The window server's toasts (RL11, A36): "needs you" on → waiting and
 *  "finished" on a settled turn end, each only for a session this client
 *  isn't looking at, after the store's 1.5 s debounce. */
const PRIMARY_TOASTER = {
  needsYou(row: ActivityRow): void {
    const label = rowAgentLabel(row)
    useToastStore.getState().addToast(
      label ? `${label} needs you` : 'An agent needs you',
      'info',
      5000,
      { label: 'View', onClick: () => navigateToRow(row) },
    )
  },
  finished(row: ActivityRow): void {
    const label = rowAgentLabel(row)
    useToastStore.getState().addToast(
      label ? `${label} has finished working` : 'An agent has finished working',
      'success',
      4000,
      { label: 'View', onClick: () => navigateToRow(row) },
    )
  },
}

/** Structural equality for the polled agents map. Lets `pollOnce` keep the
 *  existing `agents` Map reference (and avoid a spurious re-render of every
 *  subscriber) when a poll cycle yields the same set of agents. */
function agentMapsEqual(
  a: Map<string, ActiveAgent>,
  b: Map<string, ActiveAgent>,
): boolean {
  if (a.size !== b.size) return false
  for (const [id, av] of a) {
    const bv = b.get(id)
    if (
      !bv ||
      av.command !== bv.command ||
      av.status !== bv.status ||
      av.display !== bv.display ||
      av.tabId !== bv.tabId ||
      av.tabTitle !== bv.tabTitle ||
      av.groupIndex !== bv.groupIndex
    ) {
      return false
    }
  }
  return true
}

/** True when any live PTY's cwd is inside `projectPath` (exact match or a
 *  subdirectory) — i.e. the workspace currently holds a live session. Drives
 *  the Active-bar green "live session" dot. Reads `liveSessionCwds` off the
 *  active-agents store. */
export function projectHasLiveSession(liveSessionCwds: Set<string>, projectPath: string): boolean {
  if (liveSessionCwds.has(projectPath)) return true
  const prefix = projectPath.endsWith('/') ? projectPath : `${projectPath}/`
  for (const cwd of liveSessionCwds) {
    if (cwd.startsWith(prefix)) return true
  }
  return false
}

export interface ActiveAgent {
  terminalId: string
  command: string
  tabId: string
  tabTitle: string
  groupIndex: number
  /** `active` while the daemon's row says working or waiting. */
  status: 'active' | 'idle'
  /** The daemon's display for the session (idle when it has no row). */
  display: ActivityDisplay
}

// RETIRED 0.40.48 — renderer launch-failure heuristic ("Agent launch
// failed — retrying in 30s" toast + 30s triage_decide auto-retry).
//
// It inferred launch health from hook TIMING (a 'stop' within 5s of a
// 'start' stamp = dead launch), but the daemon buckets every
// UserPromptSubmit/PostToolUse as 'start' and real wake flows include
// daemon-retried infant deaths — so healthy agent-to-agent messaging
// false-positived constantly, and the auto-retry could spawn duplicate
// sessions. Launch health is the DAEMON's knowledge (it observes real
// ChildExit codes and already retries wakes); a renderer timing guess is
// the wrong layer. A genuinely dead spawn is visible in its own pane.

/** A terminal that needs to be briefly mounted off-screen to spawn its PTY */
export interface BackgroundSpawn {
  id: string
  terminalId: string
  cwd: string
  command: string
  args: string[]
}

interface ActiveAgentsState {
  agents: Map<string, ActiveAgent>
  /** Terminals waiting to be briefly mounted off-screen to spawn their PTY */
  backgroundSpawns: BackgroundSpawn[]
  /** cwds of every live PTY — drives the Active-bar "has a live session" dot.
   *  Derived from the list-running poll; a workspace is "live" when any PTY's
   *  cwd is inside it. See `projectHasLiveSession`. */
  liveSessionCwds: Set<string>

  getActiveAgentsList: () => ActiveAgent[]
  getAgentsInTab: (tabId: string) => ActiveAgent[]
  addBackgroundSpawn: (spawn: BackgroundSpawn) => void
  removeBackgroundSpawn: (id: string) => void
  /** #688 — push a live PTY's cwd into `liveSessionCwds` (from a daemon
   *  `SessionAdded` broadcast) so the Active-bar dot turns green without a
   *  poll. Idempotent; keeps the existing Set reference when the cwd is
   *  already present so subscribers don't re-render needlessly. */
  addLiveSessionCwd: (cwd: string) => void
  /** #688 — drop a cwd from `liveSessionCwds` (from a daemon
   *  `SessionRemoved` broadcast). Idempotent. */
  removeLiveSessionCwd: (cwd: string) => void

  pollOnce: () => Promise<void>
}

export const useActiveAgentsStore = create<ActiveAgentsState>((set, get) => ({
  agents: new Map(),
  backgroundSpawns: [],
  liveSessionCwds: new Set(),

  addBackgroundSpawn: (spawn: BackgroundSpawn) => {
    set((s) => ({ backgroundSpawns: [...s.backgroundSpawns, spawn] }))
    setTimeout(() => get().removeBackgroundSpawn(spawn.id), 10000)
  },
  removeBackgroundSpawn: (id: string) => {
    set((s) => ({ backgroundSpawns: s.backgroundSpawns.filter((b) => b.id !== id) }))
  },

  addLiveSessionCwd: (cwd: string) => {
    if (!cwd) return
    const prev = get().liveSessionCwds
    if (prev.has(cwd)) return
    const next = new Set(prev)
    next.add(cwd)
    set({ liveSessionCwds: next })
  },
  removeLiveSessionCwd: (cwd: string) => {
    if (!cwd) return
    const prev = get().liveSessionCwds
    if (!prev.has(cwd)) return
    const next = new Set(prev)
    next.delete(cwd)
    set({ liveSessionCwds: next })
  },

  getActiveAgentsList: () => Array.from(get().agents.values()),

  getAgentsInTab: (tabId: string) =>
    Array.from(get().agents.values()).filter((a) => a.tabId === tabId),

  pollOnce: async () => {
    const tabsState = useTabsStore.getState()

    // Collect all terminals across all tab groups
    const terminals: Array<{
      data: TerminalItemData
      tabId: string
      tabTitle: string
      groupIndex: number
    }> = []

    const groups = [tabsState.tabs, ...tabsState.extraGroups.map((g) => g.tabs)]
    for (let gi = 0; gi < groups.length; gi++) {
      for (const tab of groups[gi]) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              terminals.push({
                data: item.data as TerminalItemData,
                tabId: tab.id,
                tabTitle: tab.title,
                groupIndex: gi,
              })
            }
          }
        }
      }
    }

    const newAgents = new Map<string, ActiveAgent>()
    const { agents: oldAgents } = get()

    // Fetch every live PTY's foreground command in a SINGLE request
    // (`/cli/terminal/list-running`) instead of one
    // `/cli/terminal/foreground-cmd` request per terminal. The old
    // per-terminal `Promise.all` fired N HTTP requests on every poll
    // (~2.5s); with many open terminals that flooded the WebView's
    // network stack (WebKit ResourceRequest churn) and was a prime
    // contributor to the intermittent terminal-stall storm. `list-running`
    // computes the identical `get_foreground_command` per terminal
    // daemon-side, so this is behaviour-preserving — just one round-trip.
    let running: RunningTerminalInfo[] = []
    try {
      running = await terminalListRunning(primaryScope())
    } catch {
      // Daemon momentarily unreachable — skip this cycle's agent
      // detection rather than thrash with per-terminal retries.
    }
    const cmdByTerminal = new Map(running.map((r) => [r.terminalId, r.command]))

    // Active-bar "has a live session" dot — the set of cwds with a live
    // session. UNION of two sources because they're disjoint:
    //   - terminal/list-running → legacy TerminalManager PTYs (`running`)
    //   - agents/running        → v2 daemon-PTY agent sessions (the pinned
    //                             Chat / claude sessions — NOT in list-running)
    // The v2 set is the one that actually matters for the chat sessions we
    // reap; without it the dot would never light for a normal workspace.
    const liveCwds = new Set(running.map((r) => r.cwd))
    try {
      const agentSessions = asArray<{ cwd: string }>(
        await daemonCliGet(primaryScope(), 'agents/running'),
      )
      for (const a of agentSessions) {
        if (a?.cwd) liveCwds.add(a.cwd)
      }
    } catch {
      // Daemon momentarily unreachable — keep the legacy set for this cycle.
    }
    const prevLive = get().liveSessionCwds
    let liveChanged = prevLive.size !== liveCwds.size
    if (!liveChanged) {
      for (const c of liveCwds) {
        if (!prevLive.has(c)) { liveChanged = true; break }
      }
    }
    if (liveChanged) set({ liveSessionCwds: liveCwds })

    // What each agent is doing is the daemon's row (S5), not a guess from
    // output timing.
    const activity = activityStore(primaryScope()).getState()
    for (const t of terminals) {
      const command = cmdByTerminal.get(t.data.terminalId) ?? null
      if (command && KNOWN_AGENT_COMMANDS.has(command)) {
        const display = rowForTerminal(activity, t.data)?.display ?? 'idle'
        newAgents.set(t.data.terminalId, {
          terminalId: t.data.terminalId,
          command,
          tabId: t.tabId,
          tabTitle: t.tabTitle,
          groupIndex: t.groupIndex,
          status: isBusyDisplay(display) ? 'active' : 'idle',
          display,
        })
      }
    }

    // Only replace the `agents` Map reference when its contents actually
    // changed. Pre-fix this unconditionally set a fresh Map every poll
    // (~2.5s), forcing every component subscribing to `agents` (sidebar
    // Active section, IconRail, …) to re-render every cycle even when
    // nothing changed — sustained re-render churn that amplified the
    // terminal-stall storm.
    if (!agentMapsEqual(oldAgents, newAgents)) set({ agents: newAgents })
  },
}))

// ── Polling ─────────────────────────────────────────────────────────
let pollInterval: ReturnType<typeof setInterval> | null = null
// 0.39.39 (#675.2) — push-subscription teardowns (app-hello re-snapshot).
// Module-level so stopAgentPolling can clean them up.
let agentHelloUnsub: (() => void) | null = null
// #688 — app-level SessionAdded/SessionRemoved teardowns. These keep
// `liveSessionCwds` fresh push-style (the 2.5s poll that used to do it is
// gone). Module-level so stopAgentPolling can clean them up.
let sessionAddedUnsub: (() => void) | null = null
let sessionRemovedUnsub: (() => void) | null = null
let hooksInstallFailedUnsub: (() => void) | null = null
// S5 — the window server's activity feed and its toasts.
let activityRelease: (() => void) | null = null
let activityNotifyRelease: (() => void) | null = null

// #688 — map a session's canonical key (`agent_name`, the v2_session_map
// key, stable across its add/remove pair) → the cwd it reported. Lets a
// SessionRemoved drop the right cwd from `liveSessionCwds` ONLY when no
// other tracked session still shares that cwd (a workspace can hold several
// live PTYs in the same cwd — blindly removing on the first close would
// false-grey the dot). `pollOnce` (startup + reconnect) is the snapshot
// baseline that re-seeds liveSessionCwds + heals any drift in this map.
const _liveSessionCwdByKey = new Map<string, string>()

/** #688 — fold a daemon SessionAdded into the live-session cwd tracking.
 *  Records the key→cwd association and lights the cwd. */
function trackSessionAdded(agentName: string, cwd: string): void {
  if (!cwd) return
  _liveSessionCwdByKey.set(agentName, cwd)
  useActiveAgentsStore.getState().addLiveSessionCwd(cwd)
}

/** #688 — fold a daemon SessionRemoved into the live-session cwd tracking.
 *  Drops the key and ungreys the cwd ONLY when no other tracked session
 *  still lives in it. */
function trackSessionRemoved(agentName: string, cwdHint: string): void {
  const cwd = _liveSessionCwdByKey.get(agentName) ?? cwdHint
  _liveSessionCwdByKey.delete(agentName)
  if (!cwd) return
  for (const remaining of _liveSessionCwdByKey.values()) {
    if (remaining === cwd) return // another live session keeps it green
  }
  useActiveAgentsStore.getState().removeLiveSessionCwd(cwd)
}

export function startAgentPolling(): void {
  if (pollInterval || agentHelloUnsub || activityRelease) return
  // Initial poll — snapshot of current truth (the agents map + the
  // liveSessionCwds live-session dot).
  useActiveAgentsStore.getState().pollOnce()

  // prd-daemon-activity-and-thread-working-v1 S5 — the window server's
  // activity rows (snapshot + `activity_changed`, or an older server's
  // `session_activity_changed` as-is) and the toasts for its turn ends.
  activityRelease = attachActivity(primaryScope())
  activityNotifyRelease = registerActivityNotify(primaryScope(), {
    toaster: PRIMARY_TOASTER,
    // MS21 — the window's Active bar is the primary room: its chime reads
    // the primary room's project list.
    projects: () => useProjectsStore.getState().projects,
    root: null,
  })

  if (serverSupports('daemon-broadcasts')) {
    // 0.39.39 (#675.2) — push-primary. On every WS (re)connect we re-`pollOnce`
    // (via onAppHello) to refresh the agents map + the live-session set.
    agentHelloUnsub = onAppHello(primaryScope(), () => {
      useActiveAgentsStore.getState().pollOnce()
    })
    // prd-daemon-activity-and-thread-working-v1 A11 — the daemon installs
    // the agent CLI hooks now; when it can't (e.g. a malformed
    // ~/.claude/settings.json it refuses to overwrite), say so once.
    hooksInstallFailedUnsub = onHooksInstallFailed(primaryScope(), (e) => {
      const failures = e.failures ?? []
      if (failures.length === 0) return
      const clis = failures.map((f) => f.cli).join(', ')
      useToastStore.getState().addToast(
        `K2 couldn't install its hooks for ${clis}. Run \`k2 hooks status\` for details.`,
        'warning',
        10000,
      )
    })
    // #688 — keep the Active-bar live-session dot fresh push-style. The
    // retired 2.5s poll was the ONLY thing updating `liveSessionCwds`, so a
    // PTY opened after startup (e.g. a daemon-owned pinned chat via
    // ensure-pinned-chat → SessionAdded) never lit the dot until the next
    // reconnect re-poll. Subscribe APP-LEVEL (every workspace, not just the
    // active one) to the daemon's SessionAdded/SessionRemoved lifecycle and
    // light / ungrey the cwd directly. `pollOnce` (startup + reconnect via
    // onAppHello above) remains the snapshot baseline.
    sessionAddedUnsub = onSessionAddedApp(primaryScope(), (e) => {
      trackSessionAdded(e.agent_name, e.workspace_path)
    })
    sessionRemovedUnsub = onSessionRemovedApp(primaryScope(), (e) => {
      trackSessionRemoved(e.agent_name, e.workspace_path)
    })
  } else {
    // Fallback: legacy ~2.5s poll loop (older / remote daemon, no broadcasts).
    // Add jitter to avoid thundering-herd across multiple windows.
    const interval = 2500 + Math.floor(Math.random() * 500)
    pollInterval = setInterval(() => {
      useActiveAgentsStore.getState().pollOnce()
    }, interval)
  }

  // Helper: create a tab for a companion-spawned terminal that's already running.
  // Adds the tab to the current workspace without switching active tab.
  function createCompanionTab(terminalId: string, command: string, cwd: string) {
    const tabsStore = useTabsStore.getState()

    // Check if a tab for this terminal already exists
    const exists = tabsStore.tabs.some((t) =>
      [...t.paneGroups.values()].some((pg) =>
        pg.items.some((item) => item.type === 'terminal' && (item.data as any).terminalId === terminalId)
      )
    )
    if (exists) return

    // Save current active tab so we can restore it after addTab switches
    const currentActiveTabId = tabsStore.activeTabId
    const cmd = command.split(' ')[0] || 'shell'
    tabsStore.addTab(cwd, {
      title: `Companion: ${cmd}`,
      command: cmd,
      args: command.split(' ').slice(1),
    })

    // Override the new tab's terminal ID to connect to the existing PTY
    const updatedStore = useTabsStore.getState()
    const newTab = updatedStore.tabs[updatedStore.tabs.length - 1]
    if (newTab) {
      const pg = [...newTab.paneGroups.values()][0]
      if (pg?.items[0]?.data) {
        (pg.items[0].data as any).terminalId = terminalId
      }
      const oldPgId = pg?.id
      if (oldPgId && oldPgId !== terminalId) {
        newTab.paneGroups.delete(oldPgId)
        pg.id = terminalId
        newTab.paneGroups.set(terminalId, pg)
        newTab.mosaicTree = terminalId
      }
    }

    // Restore the previously active tab — don't switch to the new one
    if (currentActiveTabId) {
      useTabsStore.setState({ activeTabId: currentActiveTabId })
    }
  }

  // CLI-triggered launches and spawns from this computer's daemon. (The
  // Tauri `agent:lifecycle` hook event is gone: the daemon owns activity,
  // prd-daemon-activity-and-thread-working-v1 RL9/A35.)
  import('@tauri-apps/api/event').then(({ listen }) => {
    // Listen for CLI-triggered agent launch requests
    listen<{ command: string; args: string[]; cwd: string; agentName: string; worktreePath?: string }>('cli:agent-launch', async (event) => {
      const { command, args, cwd, agentName, worktreePath } = event.payload
      const tabOpts = { title: `Agent: ${agentName}`, command, args }

      // If this launch is for a worktree, create the PTY in the background
      // with the same terminal ID the Chat tab will use (agent-chat:wt:<wsId>).
      if (worktreePath) {
        // Wait briefly for sync:projects to register the new workspace
        let wsId: string | null = null
        for (let attempt = 0; attempt < 10; attempt++) {
          const projectsStore = useProjectsStore.getState()
          for (const project of projectsStore.projects) {
            const ws = project.workspaces.find((w) => w.worktreePath === worktreePath)
            if (ws) { wsId = ws.id; break }
          }
          if (wsId) break
          await new Promise((r) => setTimeout(r, 500))
        }

        if (wsId) {
          const bgTerminalId = worktreeChatId(wsId)
          try {
            const exists = await terminalExists(primaryScope(), bgTerminalId)
            if (!exists) {
              await terminalCreate(primaryScope(), { cwd, command, args, id: bgTerminalId })
            }
            // Register system-managed worktree session
            daemonCliPostQuery(primaryScope(), 'agents/lock', {
              project: cwd,
              agent: agentName,
              terminal_id: bgTerminalId,
              owner: 'system',
            }).catch(() => {})
          } catch { /* will be created when user navigates */ }
        }
        return
      }

      // For agent launches without a worktree (e.g. manager), create the PTY
      // in the background with a project-namespaced ID. The Chat tab
      // discovers it via terminal_list_running_agents when the user navigates
      // there. Project-namespacing is required so two workspaces sharing an
      // agent name don't collide on a single PTY.
      // MS3 — these Tauri events come from THIS computer's daemon and act on
      // the primary room (MS14: active-agents is primary-only), so the path
      // resolves in the primary room's own project list.
      const projectsStore = useProjectsStore.getState()
      const owningProject = projectsStore.projects.find((p) => p.path === cwd)
      if (!owningProject) {
        console.warn(
          '[cli:agent-launch] No project registered for cwd, skipping background PTY:',
          cwd,
        )
        return
      }
      const bgTerminalId = agentChatId(owningProject.id, agentName)
      try {
        const exists = await terminalExists(primaryScope(), bgTerminalId)
        if (!exists) {
          await terminalCreate(primaryScope(), {
            cwd,
            command,
            args,
            id: bgTerminalId,
          })
        }
        // Register system-managed session in DB (owner='system' so scheduler knows)
        daemonCliPostQuery(primaryScope(), 'agents/lock', {
          project: cwd,
          agent: agentName,
          terminal_id: bgTerminalId,
          owner: 'system',
        }).catch(() => {})
        // Detect and save session ID after a brief delay
        setTimeout(async () => {
          try {
            const r = await daemonCliGet<{ sessionId: string | null }>(primaryScope(), 'chat/detect-active', {
              provider: 'claude',
              project_path: cwd,
            })
            const sessionId = r.sessionId
            if (sessionId) {
              daemonCliPost(primaryScope(), 'agents/save-session-id', {
                project_path: cwd,
                agent_name: agentName,
                session_id: sessionId,
              }).catch(() => {})
            }
          } catch { /* ignore */ }
        }, 5000)
      } catch {
        // Fallback: add tab to current workspace if background creation fails
        const tabsStore = useTabsStore.getState()
        tabsStore.addTabToGroup(tabsStore.activeGroupIndex, cwd, tabOpts)
      }
    })

    // Listen for CLI-triggered sub-terminal spawn requests (multi-terminal execution)
    listen<{ agentName: string; command: string; cwd: string; title: string; wait: boolean; projectPath: string }>(
      'cli:terminal-spawn', (event) => {
        const { agentName, command, cwd, title } = event.payload
        const tabsStore = useTabsStore.getState()

        // Find the agent's existing tab (look for "Agent: <name>" title).
        // A legacy `paneGroups.…value?.panes?.some(p => p.title === …)`
        // probe was removed here: PaneGroup has `items` (which carry no
        // `title`), so the probe always evaluated to undefined/falsy.
        const agentTab = tabsStore.tabs.find((t) => t.title === `Agent: ${agentName}`)

        if (agentTab) {
          // Split within the existing agent tab
          const activeGroup = tabsStore.activeGroupIndex
          tabsStore.addTabToGroup(activeGroup, cwd, {
            title: `${agentName}: ${title}`,
            command: command.split(' ')[0],
            args: command.split(' ').slice(1),
          })
        } else {
          // No agent tab found — create new tab
          const activeGroup = tabsStore.activeGroupIndex
          tabsStore.addTabToGroup(activeGroup, cwd, {
            title: `${agentName}: ${title}`,
            command: command.split(' ')[0],
            args: command.split(' ').slice(1),
          })
        }
      }
    )

    // Companion background terminal spawn — queue for tab creation.
    // If the target workspace is active, create the tab immediately.
    // If not, queue it and create when the user switches to that workspace.
    const pendingCompanionTerminals: Array<{
      terminalId: string
      command: string
      cwd: string
      projectPath: string
    }> = []

    listen<{
      terminalId: string
      command: string
      cwd: string
      projectPath: string
      /** Set when the spawn was driven by a scheduled heartbeat fire.
       *  Tab creation is gated on the workspace's
       *  show_heartbeat_sessions flag — silent autonomous heartbeats
       *  never surface a tab unless the user opted in. Manual launches
       *  and awareness-bus wakes leave this null/undefined and always
       *  surface a tab as before. */
      heartbeatName?: string | null
    }>(
      'cli:terminal-spawn-background', async (event) => {
        const { terminalId, command, cwd, projectPath, heartbeatName } = event.payload

        // Heartbeat-driven spawn: check the workspace's preference before
        // creating a tab. Default OFF means silent autonomous mode —
        // the audit surface is the sidebar Heartbeats panel, not a tab.
        if (heartbeatName) {
          let allow = false
          try {
            allow = await invoke<boolean>(
              'k2so_workspace_get_show_heartbeat_sessions',
              { projectPath },
            )
          } catch (err) {
            console.warn(
              '[cli:terminal-spawn-background] show_heartbeat_sessions read failed; defaulting to silent:',
              err,
            )
          }
          if (!allow) {
            // Silent autonomous mode — daemon owns the PTY; no tab.
            return
          }
        }

        // Find which project this terminal belongs to
        const projects = useProjectsStore.getState().projects
        const project = projects.find((p) => cwd.startsWith(p.path))
        const activeProjectId = useProjectsStore.getState().activeProjectId

        if (project && project.id === activeProjectId) {
          // Target workspace is active — create tab immediately (without switching to it)
          createCompanionTab(terminalId, command, cwd)
        } else {
          // Queue for later — will be created when user switches to this workspace
          pendingCompanionTerminals.push({ terminalId, command, cwd, projectPath })
        }
      }
    )

    // SessionSurfaced — fired by k2so_session_set_surfaced when the
    // user (or the openHeartbeatTab path) flips an existing PTY's
    // surfaced flag from 0 → 1. Differs from cli:terminal-spawn-background
    // semantically: Spawn means "I just spawned a new PTY, here's its
    // info"; Surfaced means "this PTY is already running, mount a tab
    // on it." The tab-creation path is the same — createCompanionTab
    // attaches by terminal_id rather than spawning fresh.
    // See `.k2so/prds/heartbeat-active-session-tracking.md`.
    listen<{
      terminalId: string | null
      command: string | null
      args: string[] | null
      projectPath: string
      agentName: string
      heartbeatName: string | null
      attachAgentName?: string | null
    }>(
      'session:surfaced', async (event) => {
        const { terminalId, command, projectPath, heartbeatName } = event.payload
        const attachAgentName = event.payload.attachAgentName ?? null
        const args = event.payload.args ?? []
        if (!terminalId || !command) return

        // Bail if the user already has a tab for this surfaced
        // session (cross-window race or duplicate event); just focus
        // it instead of creating a duplicate.
        const tabsStore = useTabsStore.getState()
        const existing = tabsStore.tabs.find((t) =>
          [...t.paneGroups.values()].some((pg) =>
            pg.items.some(
              (item) =>
                item.type === 'terminal' &&
                (item.data as any).terminalId === terminalId,
            ),
          ),
        )
        if (existing) {
          useTabsStore.setState({ activeTabId: existing.id })
          return
        }

        // MS3 — primary room's own list (see `cli:agent-launch` above).
        const projects = useProjectsStore.getState().projects
        const project = projects.find((p) => p.path === projectPath)
        const activeProjectId = useProjectsStore.getState().activeProjectId
        const cwd = project?.path ?? projectPath
        if (!project || project.id !== activeProjectId) {
          pendingCompanionTerminals.push({ terminalId, command, cwd, projectPath })
          return
        }

        // Build the tab synchronously with all fields stamped from
        // the start. Using addTab + post-mutation here causes a race:
        // TerminalPane mounts and calls /cli/sessions/v2/spawn with
        // its default `tab-${terminalId}` agent_name BEFORE the
        // post-stamp can override it with `attachAgentName`. The
        // wrong agent_name gives the daemon no existing session to
        // reuse → fresh blank PTY. By placing all the fields onto
        // the new tab in a single setState before TerminalPane
        // mounts, attachAgentName is on the data when /cli/sessions/v2/spawn
        // is called and the daemon returns the existing session
        // (`reused: true`).
        // See `.k2so/prds/heartbeat-active-session-tracking.md`.
        const cmd = command.split(' ')[0] || 'shell'
        const tabId = crypto.randomUUID()
        const paneGroupId = terminalId
        const itemId = crypto.randomUUID()
        const title = heartbeatName
          ? `${heartbeatName} (heartbeat)`
          : `Companion: ${cmd}`
        const newTab = {
          id: tabId,
          title,
          isSystemAgent: false,
          paneGroups: new Map([
            [
              paneGroupId,
              {
                id: paneGroupId,
                items: [
                  {
                    id: itemId,
                    type: 'terminal' as const,
                    data: {
                      terminalId,
                      cwd,
                      command: cmd,
                      args,
                      renderer: 'kessel' as const,
                      spawnedAt: performance.now(),
                      heartbeatName: heartbeatName ?? undefined,
                      projectPath,
                      surfacedAgentName: event.payload.agentName,
                      attachAgentName: attachAgentName ?? undefined,
                    },
                  },
                ],
                activeItemIndex: 0,
              },
            ],
          ]),
          mosaicTree: paneGroupId,
        } as any
        useTabsStore.setState((s) => ({
          tabs: [...s.tabs, newTab],
          // Auto-focus the surfaced tab. SessionSurfaced fires only
          // on explicit user action (heartbeat-row click → set_surfaced),
          // so the user clearly wants to look at the heartbeat now.
          // Diverges from createCompanionTab's convention of preserving
          // the previous active tab — that path is for autonomous
          // background surfacing, this one is user-summoned.
          activeTabId: tabId,
        }))
      },
    )

    // 0.38.0 commit 6 — symmetric counterpart to `session:surfaced`.
    // Fires when any window flips a workspace session's `surfaced`
    // flag 1 → 0 (close-as-minimize). Every viewer drops the
    // corresponding tab from its UI; the daemon-owned PTY stays
    // alive in v2_session_map. Idempotent: no-op if the tab isn't
    // present (e.g. this window already minimized locally).
    listen<{
      terminalId: string | null
      projectPath: string
      agentName: string
      heartbeatName: string | null
      attachAgentName?: string | null
    }>(
      'session:unsurfaced', (event) => {
        const { terminalId, agentName } = event.payload
        const heartbeatName = event.payload.heartbeatName
        const tabsStore = useTabsStore.getState()
        // Match by either terminalId on the item data OR by
        // surfacedAgentName / heartbeatName on the tab — covers both
        // stamped-metadata and cross-reference cases of the original
        // closeTerminalForRenderer detection.
        const matchTabId = tabsStore.tabs.find((t) =>
          [...t.paneGroups.values()].some((pg) =>
            pg.items.some((item) => {
              if (item.type !== 'terminal') return false
              const d = item.data as any
              if (terminalId && d.terminalId === terminalId) return true
              if (d.surfacedAgentName && d.surfacedAgentName === agentName) return true
              if (heartbeatName && d.heartbeatName === heartbeatName) return true
              return false
            }),
          ),
        )?.id
        if (!matchTabId) return
        useTabsStore.setState((s) => {
          const nextTabs = s.tabs.filter((t) => t.id !== matchTabId)
          // If we just dropped the active tab, point active at whatever
          // tab is left (or null if the group is empty).
          const nextActive = s.activeTabId === matchTabId
            ? (nextTabs[0]?.id ?? null)
            : s.activeTabId
          return { tabs: nextTabs, activeTabId: nextActive }
        })
      },
    )

    // Watch for workspace switches — flush any pending companion terminals
    useProjectsStore.subscribe((state, prevState) => {
      if (state.activeProjectId && state.activeProjectId !== prevState.activeProjectId) {
        const project = state.projects.find((p) => p.id === state.activeProjectId)
        if (!project) return

        // Find and create tabs for any pending terminals in this workspace
        const toCreate = pendingCompanionTerminals.filter((t) => t.cwd.startsWith(project.path))
        for (const t of toCreate) {
          // Brief delay to let restoreWorkspace finish
          setTimeout(() => createCompanionTab(t.terminalId, t.command, t.cwd), 300)
        }
        // Remove created terminals from pending list
        for (let i = pendingCompanionTerminals.length - 1; i >= 0; i--) {
          if (pendingCompanionTerminals[i].cwd.startsWith(project.path)) {
            pendingCompanionTerminals.splice(i, 1)
          }
        }
      }
    })

    // Listen for CLI-triggered AI Commit requests
    listen<{
      projectPath: string
      includeMerge: boolean
      message: string
      gitContext: {
        branch: string
        status: string
        diffStat: string
        stagedStat: string
        recentLog: string
      }
    }>('cli:ai-commit', (event) => {
      const { projectPath, includeMerge, message, gitContext } = event.payload
      // Resolve the default agent through the one seam (id-first,
      // legacy-token tolerant, first-enabled fallback). The target
      // project's own default (Slice 1) takes precedence once it exists.
      // MS3 — primary room's own list (see `cli:agent-launch` above).
      const project = useProjectsStore
        .getState()
        .projects.find(
          (p) =>
            p.path === projectPath ||
            p.workspaces.some((w) => w.worktreePath === projectPath),
        )
      const resolved = resolveAgentCommand(
        usePresetsStore.getState().presets,
        useSettingsStore.getState().defaultAgent,
        readProjectDefaultAgent(project),
      )
      if (!resolved) return

      const { command, args } = resolved

      // Build a rich prompt with git context so the agent has immediate visibility
      const parts: string[] = []

      if (message) {
        parts.push(message)
      } else {
        parts.push('Create a commit for the changes in this repository.')
      }

      parts.push('')
      parts.push('## Current State')
      parts.push(`Branch: ${gitContext.branch || 'unknown'}`)

      if (gitContext.stagedStat) {
        parts.push('')
        parts.push('### Staged Changes')
        parts.push('```')
        parts.push(gitContext.stagedStat)
        parts.push('```')
      }

      if (gitContext.diffStat) {
        parts.push('')
        parts.push('### Unstaged Changes')
        parts.push('```')
        parts.push(gitContext.diffStat)
        parts.push('```')
      }

      if (gitContext.status) {
        parts.push('')
        parts.push('### git status')
        parts.push('```')
        parts.push(gitContext.status)
        parts.push('```')
      }

      if (gitContext.recentLog) {
        parts.push('')
        parts.push('### Recent Commits (for style reference)')
        parts.push('```')
        parts.push(gitContext.recentLog)
        parts.push('```')
      }

      parts.push('')
      parts.push('## Instructions')
      parts.push('1. Review the diff carefully with `git diff` and `git diff --cached`')
      parts.push('2. Stage any unstaged files that should be included (use `git add <file>`, not `git add -A`)')
      parts.push('3. Write a clear, concise commit message that explains the **why**, matching the style of recent commits above')
      parts.push('4. Commit the changes')

      if (includeMerge) {
        parts.push('5. After committing, merge this branch into main and resolve any conflicts')
      }

      const prompt = parts.join('\n')

      const tabsStore = useTabsStore.getState()
      const activeGroup = tabsStore.activeGroupIndex
      tabsStore.addTabToGroup(activeGroup, projectPath, {
        title: includeMerge ? 'AI Commit & Merge' : 'AI Commit',
        command,
        args: [...args, prompt]
      })
    })
  })
}

export function stopAgentPolling(): void {
  if (pollInterval) {
    clearInterval(pollInterval)
    pollInterval = null
  }
  if (agentHelloUnsub) {
    agentHelloUnsub()
    agentHelloUnsub = null
  }
  if (hooksInstallFailedUnsub) {
    hooksInstallFailedUnsub()
    hooksInstallFailedUnsub = null
  }
  if (sessionAddedUnsub) {
    sessionAddedUnsub()
    sessionAddedUnsub = null
  }
  if (sessionRemovedUnsub) {
    sessionRemovedUnsub()
    sessionRemovedUnsub = null
  }
  if (activityNotifyRelease) {
    activityNotifyRelease()
    activityNotifyRelease = null
  }
  if (activityRelease) {
    activityRelease()
    activityRelease = null
  }
  _liveSessionCwdByKey.clear()
}

// ── #625: host-switch reset of agent pane state ─────────────────────────
//
// `agents` / `liveSessionCwds` (store) and the window server's activity view
// are keyed by sessions and cwds of the server the window WAS connected to.
// As singletons they survive the `<App key={hostKey}>` remount on a host
// switch, so after connecting to a REMOTE daemon the local box's spinners
// would paint against the wrong host.
//
// On a real host CHANGE, wipe all of it: the new host's own snapshot (the
// next hello) repopulates it.
export function __resetAgentStateForHostSwitch(): void {
  // #688 — the live-session cwd tracking is keyed by the LOCAL host's
  // sessions; wipe it on a host change so the new host's snapshot poll
  // re-seeds `liveSessionCwds` cleanly (and a stale local key can't keep a
  // remote cwd falsely green).
  _liveSessionCwdByKey.clear()
  resetActivity(primaryScope())
  useActiveAgentsStore.setState({
    agents: new Map(),
    backgroundSpawns: [],
    liveSessionCwds: new Set(),
  })
}

onActiveHostChange(() => {
  __resetAgentStateForHostSwitch()
})
