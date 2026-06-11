import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  terminalCreate,
  terminalExists,
  terminalListRunning,
  type RunningTerminalInfo,
} from '@/lib/terminal-daemon'
import { useTabsStore, type TerminalItemData } from './tabs'
import { useToastStore } from './toast'
import { useProjectsStore } from './projects'
import { usePresetsStore, parseCommand } from './presets'
import { useSettingsStore } from './settings'
import { KNOWN_AGENT_COMMANDS, AGENT_IDLE_THRESHOLD_MS } from '@shared/constants'
import { agentChatId, worktreeChatId, parseTerminalId } from '@/lib/terminal-id'
// #625 — reset agent pane state on a host switch.
import { onActiveHostChange } from '@/stores/connect-host'
import { serverSupports } from '@/lib/server-capabilities'
import {
  onAgentStatusChanged,
  onAppHello,
  onSessionAddedApp,
  onSessionRemovedApp,
} from '@/stores/session-events'

export type PaneStatus = 'idle' | 'working' | 'permission' | 'review'

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
      av.hookStatus !== bv.hookStatus ||
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
  status: 'active' | 'idle'
  /** Hook-based pane status (more accurate than polling) */
  hookStatus: PaneStatus
}

/** Last time the Tauri `agent:lifecycle` hook fired for a pane. Used by the
 *  poll-based cleanup to avoid clobbering hook-driven 'working' states while
 *  hooks are actively reporting. A long grace covers quiet Claude turns
 *  (pure thinking, no tool calls) where no hook fires until Stop. */
const _hookEventAt = new Map<string, number>()
const HOOK_TRUST_GRACE_MS = 120_000
const OUTPUT_TRUST_GRACE_MS = 3_000

/** Track agent start times for launch failure detection (paneId → timestamp) */
const _agentStartTimes = new Map<string, number>()
/** Track failed launches to avoid infinite retry loops (paneId → retry count) */
const _launchRetries = new Map<string, number>()
/** Track pending retry timeouts so they can be cancelled on cleanup */
const _retryTimeouts = new Set<ReturnType<typeof setTimeout>>()
const LAUNCH_FAILURE_THRESHOLD_MS = 5000
const MAX_LAUNCH_RETRIES = 1

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
  outputTimestamps: Map<string, number>
  /** Hook-based pane statuses keyed by paneId (terminalId) */
  paneStatuses: Map<string, PaneStatus>
  /** Maps paneId → projectId so we know which project an agent belongs to */
  paneProjectMap: Map<string, string>
  /** Terminals waiting to be briefly mounted off-screen to spawn their PTY */
  backgroundSpawns: BackgroundSpawn[]
  /** cwds of every live PTY — drives the Active-bar "has a live session" dot.
   *  Derived from the list-running poll; a workspace is "live" when any PTY's
   *  cwd is inside it. See `projectHasLiveSession`. */
  liveSessionCwds: Set<string>

  hasActiveAgents: () => boolean
  getActiveAgentsList: () => ActiveAgent[]
  getAgentsInTab: (tabId: string) => ActiveAgent[]
  isTerminalRunningAgent: (terminalId: string) => boolean
  getPaneStatus: (paneId: string) => PaneStatus
  getAggregateStatus: () => PaneStatus
  getProjectStatus: (projectId: string) => PaneStatus
  recordOutput: (terminalId: string) => void
  recordTitleActivity: (paneId: string, isWorking: boolean) => void
  bindPaneProject: (paneId: string, projectId: string) => void
  handleLifecycleEvent: (paneId: string, tabId: string, eventType: string) => void
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
  outputTimestamps: new Map(),
  paneStatuses: new Map(),
  paneProjectMap: new Map(),
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

  hasActiveAgents: () => {
    // Check both polling-detected agents and hook-detected working panes
    const { agents, paneStatuses } = get()
    if (agents.size > 0) return true
    for (const status of paneStatuses.values()) {
      if (status === 'working' || status === 'permission') return true
    }
    return false
  },

  getActiveAgentsList: () => Array.from(get().agents.values()),

  getAgentsInTab: (tabId: string) =>
    Array.from(get().agents.values()).filter((a) => a.tabId === tabId),

  isTerminalRunningAgent: (terminalId: string) => {
    if (get().agents.has(terminalId)) return true
    const hookStatus = get().paneStatuses.get(terminalId)
    return hookStatus === 'working' || hookStatus === 'permission'
  },

  getPaneStatus: (paneId: string) => get().paneStatuses.get(paneId) ?? 'idle',

  /** Get the highest-priority agent status for a specific project. */
  getProjectStatus: (projectId: string): PaneStatus => {
    const { paneStatuses, paneProjectMap } = get()
    let hasWorking = false
    let hasPermission = false
    let hasReview = false
    for (const [paneId, status] of paneStatuses) {
      if (paneProjectMap.get(paneId) === projectId) {
        if (status === 'permission') hasPermission = true
        else if (status === 'working') hasWorking = true
        else if (status === 'review') hasReview = true
      }
    }
    if (hasPermission) return 'permission'
    if (hasWorking) return 'working'
    if (hasReview) return 'review'
    return 'idle'
  },

  /** Get the highest-priority agent status across all panes. */
  getAggregateStatus: (): PaneStatus => {
    const { paneStatuses } = get()
    let hasWorking = false
    let hasPermission = false
    let hasReview = false
    for (const status of paneStatuses.values()) {
      if (status === 'permission') hasPermission = true
      else if (status === 'working') hasWorking = true
      else if (status === 'review') hasReview = true
    }
    // Priority: permission > working > review > idle
    if (hasPermission) return 'permission'
    if (hasWorking) return 'working'
    if (hasReview) return 'review'
    return 'idle'
  },

  recordOutput: (terminalId: string) => {
    get().outputTimestamps.set(terminalId, Date.now())
  },

  /**
   * Light-touch state update from terminal title signals (braille-spinner
   * prefix = working, ✳-family prefix = idle). Only flips between
   * idle ↔ working so it never clobbers 'permission' or 'review' — those
   * come from the Tauri lifecycle hook and have higher priority.
   */
  recordTitleActivity: (paneId: string, isWorking: boolean) => {
    const { paneStatuses, paneProjectMap } = get()
    const current = paneStatuses.get(paneId) ?? 'idle'
    if (current === 'permission' || current === 'review') return
    const next: PaneStatus = isWorking ? 'working' : 'idle'
    if (current === next) return
    const newStatuses = new Map(paneStatuses)
    newStatuses.set(paneId, next)
    set({ paneStatuses: newStatuses })

    // Bind paneId → activeProjectId on the first 'working' transition
    // so getProjectStatus() can attribute the spinner to a workspace
    // (drives the sidebar Active section + IconRail dots). Mirrors
    // what handleLifecycleEvent does on a 'start' hook event — but
    // for v2 panes whose working state comes from terminal-title
    // OSC events rather than from agent lifecycle hooks, this is the
    // only path that populates the map. Without it, paneStatuses
    // says 'working' but no project owns the spinner.
    if (isWorking && !paneProjectMap.has(paneId)) {
      const ps = useProjectsStore.getState()
      // P1.A — a pinned-Chat pane (terminalId `agent-chat:<projectId>`)
      // must attribute its working state to ITS OWN project, not to
      // whatever workspace the user happens to be viewing when the first
      // braille tick fires. The pinned Chat has no lifecycle hook, so this
      // title-activity path was the only binder — and it latched to
      // `activeProjectId`, which mis-bound the spinner whenever the user
      // was looking at a different workspace. Parse the paneId: if it's an
      // agent-chat id, use its embedded projectId; only fall back to
      // `activeProjectId` for non-agent-chat panes (e.g. plain Cmd+T
      // terminals that legitimately belong to the foreground workspace).
      const parsed = parseTerminalId(paneId)
      const ownProjectId =
        parsed?.kind === 'agent_chat' ? parsed.projectId : null
      const boundProjectId = ownProjectId ?? ps.activeProjectId
      if (boundProjectId) {
        const newPaneProjectMap = new Map(paneProjectMap)
        newPaneProjectMap.set(paneId, boundProjectId)
        set({ paneProjectMap: newPaneProjectMap })
        // Also touches lastInteractionAt → 24h Active Bar tenure.
        ps.touchInteraction(boundProjectId)
      }
    }
  },

  /**
   * P1.A — register a pane→project mapping UPFRONT, before any
   * title-activity signal can race. Called by AgentChatPane on mount so
   * the pinned-Chat pane's `paneProjectMap` entry exists the moment its
   * PTY is live; this guarantees `getProjectStatus(ownProject)` lights
   * the spinner on the correct workspace even if the first braille tick
   * fires while the user is viewing a different workspace. Idempotent:
   * never overwrites an existing binding (a lifecycle 'start' hook may
   * have already bound a more-specific project for the same pane).
   */
  bindPaneProject: (paneId: string, projectId: string) => {
    if (!paneId || !projectId) return
    const { paneProjectMap } = get()
    if (paneProjectMap.get(paneId) === projectId) return
    const newPaneProjectMap = new Map(paneProjectMap)
    newPaneProjectMap.set(paneId, projectId)
    set({ paneProjectMap: newPaneProjectMap })
  },

  handleLifecycleEvent: (paneId: string, _tabId: string, eventType: string) => {
    const toast = useToastStore.getState()
    const { paneStatuses } = get()
    const newStatuses = new Map(paneStatuses)

    // Record the hook fire so the poll-based cleanup doesn't race us.
    _hookEventAt.set(paneId, Date.now())

    if (eventType === 'start') {
      newStatuses.set(paneId, 'working')
      _agentStartTimes.set(paneId, Date.now())
      // Record which project this pane belongs to
      const ps = useProjectsStore.getState()
      if (ps.activeProjectId) {
        const newPaneProjectMap = new Map(get().paneProjectMap)
        newPaneProjectMap.set(paneId, ps.activeProjectId)
        set({ paneProjectMap: newPaneProjectMap })
        // Touch interaction on the active project — this triggers Active Bar
        ps.touchInteraction(ps.activeProjectId)
      }
    } else if (eventType === 'permission') {
      // Skip duplicate permission toast if already in permission state
      const currentStatus = paneStatuses.get(paneId)
      newStatuses.set(paneId, 'permission')
      if (currentStatus === 'permission') {
        set({ paneStatuses: newStatuses })
        return
      }
      // Notify user that agent needs attention
      toast.addToast(
        'An agent needs your permission',
        'info',
        5000,
        {
          label: 'View',
          onClick: () => {
            // Find which tab contains this pane and switch to it
            const tabsState = useTabsStore.getState()
            for (const tab of tabsState.tabs) {
              if (tab.paneGroups.has(paneId)) {
                tabsState.setActiveTab(tab.id)
                break
              }
            }
          },
        }
      )
    } else if (eventType === 'stop') {
      // Skip if already in stop/review/idle state (avoid duplicate toast from multiple stop events)
      const currentStatus = paneStatuses.get(paneId)
      if (currentStatus === 'review' || currentStatus === 'idle') {
        set({ paneStatuses: newStatuses })
        return
      }

      // Check if the pane's tab is currently active
      const tabsState = useTabsStore.getState()
      let isInActiveTab = false
      for (const tab of tabsState.tabs) {
        if (tab.id === tabsState.activeTabId && tab.paneGroups.has(paneId)) {
          isInActiveTab = true
          break
        }
      }
      newStatuses.set(paneId, isInActiveTab ? 'idle' : 'review')

      if (!isInActiveTab) {
        toast.addToast(
          'An agent has finished working',
          'success',
          4000,
          {
            label: 'View',
            onClick: () => {
              for (const tab of tabsState.tabs) {
                if (tab.paneGroups.has(paneId)) {
                  tabsState.setActiveTab(tab.id)
                  // Clear review status when user navigates to it
                  const statuses = new Map(get().paneStatuses)
                  statuses.set(paneId, 'idle')
                  set({ paneStatuses: statuses })
                  break
                }
              }
            },
          }
        )
      }
    }

    set({ paneStatuses: newStatuses })

    // Launch failure detection: if agent stopped within 5s of starting, retry once
    if (eventType === 'stop') {
      const startTime = _agentStartTimes.get(paneId)
      _agentStartTimes.delete(paneId)
      if (startTime && (Date.now() - startTime) < LAUNCH_FAILURE_THRESHOLD_MS) {
        const retries = _launchRetries.get(paneId) || 0
        if (retries < MAX_LAUNCH_RETRIES) {
          _launchRetries.set(paneId, retries + 1)
          const projectId = get().paneProjectMap.get(paneId)
          if (projectId) {
            const project = useProjectsStore.getState().projects.find(p => p.id === projectId)
            if (project) {
              console.warn(`[agent-launch] Agent in ${project.name} failed within ${LAUNCH_FAILURE_THRESHOLD_MS}ms — retrying in 30s (attempt ${retries + 1})`)
              toast.addToast('Agent launch failed — retrying in 30s', 'warning', 5000)
              const retryTimer = setTimeout(() => {
                _retryTimeouts.delete(retryTimer)
                invoke('k2so_agents_triage_decide', { projectPath: project.path }).catch(() => {})
              }, 30000)
              _retryTimeouts.add(retryTimer)
            }
          }
          return // Don't proceed to normal retriage
        } else {
          _launchRetries.delete(paneId)
          console.error(`[agent-launch] Agent launch failed after ${MAX_LAUNCH_RETRIES} retries`)
          toast.addToast('Agent launch failed — check agent configuration', 'error', 8000)
        }
      } else {
        _launchRetries.delete(paneId)
      }
    }

    // RETIRED 0.36.3 — legacy auto-retriage loop.
    //
    // Pre-0.36.x model: every Claude-session 'stop' event triggered a
    // renderer-driven triage that read `.k2so/agents/<name>/heartbeat.json`
    // (legacy per-agent config) and immediately re-spawned the agent in a
    // new tab. The loop was self-perpetuating — agent ends, retriage,
    // spawn, agent ends, retriage… autoBackoff slowed it but never
    // stopped it — and it bypassed `projects.heartbeat_mode='off'`
    // because it called `triage_decide` (legacy) instead of
    // `scheduler_tick` (gated). This is what was firing wakes against
    // workspaces with no DB heartbeat rows (Cortana, etc.).
    //
    // Replaced by `agent_heartbeats` (DB-backed scheduled heartbeats
    // owned by the workspace, fired by the daemon's heartbeat tick).
    // Sessions now end and stay ended until their next scheduled fire.
  },

  pollOnce: async () => {
    const tabsState = useTabsStore.getState()

    // Collect all terminals across all tab groups
    const terminals: Array<{
      terminalId: string
      tabId: string
      tabTitle: string
      groupIndex: number
    }> = []

    // Group 0
    for (const tab of tabsState.tabs) {
      for (const [, pg] of tab.paneGroups) {
        for (const item of pg.items) {
          if (item.type === 'terminal') {
            const data = item.data as TerminalItemData
            terminals.push({
              terminalId: data.terminalId,
              tabId: tab.id,
              tabTitle: tab.title,
              groupIndex: 0,
            })
          }
        }
      }
    }

    // Extra groups
    for (let gi = 0; gi < tabsState.extraGroups.length; gi++) {
      const group = tabsState.extraGroups[gi]
      for (const tab of group.tabs) {
        for (const [, pg] of tab.paneGroups) {
          for (const item of pg.items) {
            if (item.type === 'terminal') {
              const data = item.data as TerminalItemData
              terminals.push({
                terminalId: data.terminalId,
                tabId: tab.id,
                tabTitle: tab.title,
                groupIndex: gi + 1,
              })
            }
          }
        }
      }
    }

    // Poll each terminal for its foreground command
    const newAgents = new Map<string, ActiveAgent>()

    const now = Date.now()
    const { agents: oldAgents, outputTimestamps } = get()

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
      running = await terminalListRunning()
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
      const agentSessions = await daemonCliGet<Array<{ cwd: string }>>('agents/running')
      for (const a of agentSessions) liveCwds.add(a.cwd)
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

    for (const t of terminals) {
      const command = cmdByTerminal.get(t.terminalId) ?? null
      if (command && KNOWN_AGENT_COMMANDS.has(command)) {
        const lastOutput = outputTimestamps.get(t.terminalId) ?? 0
        const status: 'active' | 'idle' = (now - lastOutput < AGENT_IDLE_THRESHOLD_MS) ? 'active' : 'idle'

        newAgents.set(t.terminalId, {
          terminalId: t.terminalId,
          command,
          tabId: t.tabId,
          tabTitle: t.tabTitle,
          groupIndex: t.groupIndex,
          status,
          hookStatus: get().paneStatuses.get(t.terminalId) ?? 'idle',
        })
      }
    }

    // Detect transitions and fire toasts
    const toast = useToastStore.getState()

    // Only fire poll-based toasts if the hook system is NOT active for this pane.
    // When hooks are working, handleLifecycleEvent handles all toasts.
    const { paneStatuses } = get()

    for (const [terminalId, newAgent] of newAgents) {
      const oldAgent = oldAgents.get(terminalId)
      if (oldAgent?.status === 'active' && newAgent.status === 'idle') {
        if (!paneStatuses.has(terminalId)) {
          const { tabId, groupIndex } = newAgent
          toast.addToast(
            `${newAgent.command} is waiting for input in "${newAgent.tabTitle}"`,
            'info',
            5000,
            {
              label: 'Switch to tab',
              onClick: () => useTabsStore.getState().setActiveTabInGroup(groupIndex, tabId),
            }
          )
        }
      }
    }

    for (const [terminalId, oldAgent] of oldAgents) {
      if (!newAgents.has(terminalId) && oldAgent.status === 'active') {
        if (!paneStatuses.has(terminalId)) {
          const { tabId, groupIndex } = oldAgent
          toast.addToast(
            `${oldAgent.command} finished in "${oldAgent.tabTitle}"`,
            'success',
            4000,
            {
              label: 'Switch to tab',
              onClick: () => useTabsStore.getState().setActiveTabInGroup(groupIndex, tabId),
            }
          )
        }
      }
    }

    // Clean up output timestamps and pane statuses for terminals no longer running an agent.
    // This handles the case where the user interrupts (Esc) and the hook 'stop' event never fires.
    const cleanedStatuses = new Map(paneStatuses)
    let statusesChanged = false
    for (const terminalId of outputTimestamps.keys()) {
      if (newAgents.has(terminalId)) continue
      // Preserve the timestamp for panes with an active hook-driven
      // status — it's the OUTPUT_TRUST_GRACE_MS signal that keeps the
      // working state alive through the cleanup branch below. v2
      // (Alacritty) panes never appear in `newAgents` because
      // `terminal_get_foreground_command` only sees legacy
      // TerminalManager sessions; without this exemption their
      // 'working' state would get clobbered on every poll cycle and
      // the braille spinner would never stay lit. Hook-driven legacy
      // panes are unaffected (they're already in newAgents via the
      // KNOWN_AGENT_COMMANDS check).
      if (paneStatuses.has(terminalId)) continue
      outputTimestamps.delete(terminalId)
    }
    const cleanupNow = Date.now()
    for (const [paneId, status] of paneStatuses) {
      if ((status === 'working' || status === 'permission') && !newAgents.has(paneId)) {
        // The foreground command isn't a known agent — but that fires
        // transiently during Claude's tool-use (child process briefly
        // runs `bash`, `rg`, etc.). Only clear when *both* trust signals
        // have gone quiet: hooks haven't fired in a long time AND output
        // isn't flowing. Otherwise we clobber a legitimate working state.
        const hookAge = cleanupNow - (_hookEventAt.get(paneId) ?? 0)
        const outputAge = cleanupNow - (outputTimestamps.get(paneId) ?? 0)
        if (hookAge < HOOK_TRUST_GRACE_MS) continue
        if (outputAge < OUTPUT_TRUST_GRACE_MS) continue
        cleanedStatuses.set(paneId, 'idle')
        _hookEventAt.delete(paneId)
        statusesChanged = true
      }
    }

    // Only replace the `agents` Map reference when its contents actually
    // changed. Pre-fix this unconditionally set a fresh Map every poll
    // (~2.5s), forcing every component subscribing to `agents` (sidebar
    // Active section, IconRail, …) to re-render every cycle even when
    // nothing changed — sustained re-render churn that amplified the
    // terminal-stall storm. (`paneStatuses` already had this guard.)
    const agentsChanged = !agentMapsEqual(oldAgents, newAgents)
    if (agentsChanged && statusesChanged) {
      set({ agents: newAgents, paneStatuses: cleanedStatuses })
    } else if (agentsChanged) {
      set({ agents: newAgents })
    } else if (statusesChanged) {
      set({ paneStatuses: cleanedStatuses })
    }
  },
}))

// ── Polling ─────────────────────────────────────────────────────────
let pollInterval: ReturnType<typeof setInterval> | null = null
let hookUnlisten: (() => void) | null = null
// 0.39.39 (#675.2) — push-subscription teardowns (agent_status_changed +
// app-hello re-snapshot). Module-level so stopAgentPolling can clean them up.
let agentStatusUnsub: (() => void) | null = null
let agentHelloUnsub: (() => void) | null = null
// #688 — app-level SessionAdded/SessionRemoved teardowns. These keep
// `liveSessionCwds` fresh push-style (the 2.5s poll that used to do it is
// gone). Module-level so stopAgentPolling can clean them up.
let sessionAddedUnsub: (() => void) | null = null
let sessionRemovedUnsub: (() => void) | null = null

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
  if (pollInterval || agentStatusUnsub) return
  // Initial poll — snapshot of current truth (paneStatuses derived from
  // foreground commands + liveSessionCwds for the live-session dot).
  useActiveAgentsStore.getState().pollOnce()

  if (serverSupports('daemon-broadcasts')) {
    // 0.39.39 (#675.2) — push-primary. The daemon emits `agent_status_changed`
    // from its lifecycle-hook chokepoint (the canonical source for the same
    // start/stop/permission buckets the Tauri `agent:lifecycle` event carries),
    // so the ~2.5s poll that recomputed working/idle is GONE. We map each
    // broadcast through `handleLifecycleEvent` — the exact path the Tauri hook
    // uses — so spinner/toast/launch-failure logic is unchanged. The initial
    // `pollOnce` above still seeds `liveSessionCwds`; on every WS (re)connect
    // we re-`pollOnce` (via onAppHello) to backfill any missed transitions +
    // refresh the live-session set.
    agentStatusUnsub = onAgentStatusChanged((e) => {
      useActiveAgentsStore.getState().handleLifecycleEvent(e.paneId, e.tabId, e.status)
    })
    agentHelloUnsub = onAppHello(() => {
      useActiveAgentsStore.getState().pollOnce()
    })
    // #688 — keep the Active-bar live-session dot fresh push-style. The
    // retired 2.5s poll was the ONLY thing updating `liveSessionCwds`, so a
    // PTY opened after startup (e.g. a daemon-owned pinned chat via
    // ensure-pinned-chat → SessionAdded) never lit the dot until the next
    // reconnect re-poll. Subscribe APP-LEVEL (every workspace, not just the
    // active one) to the daemon's SessionAdded/SessionRemoved lifecycle and
    // light / ungrey the cwd directly. `pollOnce` (startup + reconnect via
    // onAppHello above) remains the snapshot baseline.
    sessionAddedUnsub = onSessionAddedApp((e) => {
      trackSessionAdded(e.agent_name, e.workspace_path)
    })
    sessionRemovedUnsub = onSessionRemovedApp((e) => {
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

  // Listen for hook-based lifecycle events from the Rust notification server
  import('@tauri-apps/api/event').then(({ listen }) => {
    listen<{ paneId: string; tabId: string; eventType: string }>('agent:lifecycle', (event) => {
      const { paneId, tabId, eventType } = event.payload
      useActiveAgentsStore.getState().handleLifecycleEvent(paneId, tabId, eventType)
    }).then((fn) => {
      hookUnlisten = fn
    })

    // Surface hook-injection failures — previously these were debug-only
    // log lines, so users never learned that e.g. a malformed
    // ~/.claude/settings.json had silently broken their spinner pipeline.
    // One toast per startup, listing which CLIs failed.
    listen<{ failures: Array<{ cli: string; error: string }> }>('hook-injection-failed', (event) => {
      const failures = event.payload?.failures ?? []
      if (failures.length === 0) return
      const clis = failures.map((f) => f.cli).join(', ')
      useToastStore.getState().addToast(
        `Hook injection failed for ${clis} — run \`k2so hooks status\` for details`,
        'warning',
        10000,
      )
    })

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
            const exists = await terminalExists(bgTerminalId)
            if (!exists) {
              await terminalCreate({ cwd, command, args, id: bgTerminalId })
            }
            // Register system-managed worktree session
            daemonCliGet('agents/lock', {
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
        const exists = await terminalExists(bgTerminalId)
        if (!exists) {
          await terminalCreate({
            cwd,
            command,
            args,
            id: bgTerminalId,
          })
        }
        // Register system-managed session in DB (owner='system' so scheduler knows)
        daemonCliGet('agents/lock', {
          project: cwd,
          agent: agentName,
          terminal_id: bgTerminalId,
          owner: 'system',
        }).catch(() => {})
        // Detect and save session ID after a brief delay
        setTimeout(async () => {
          try {
            const r = await daemonCliGet<{ sessionId: string | null }>('chat/detect-active', {
              provider: 'claude',
              project_path: cwd,
            })
            const sessionId = r.sessionId
            if (sessionId) {
              daemonCliPost('agents/save-session-id', {
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

        // Find the agent's existing tab (look for "Agent: <name>" title)
        const agentTab = tabsStore.tabs.find((t) =>
          t.title === `Agent: ${agentName}` || t.paneGroups.values().next().value?.panes?.some(
            (p: any) => p.title === `Agent: ${agentName}`
          )
        )

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
                      renderer: 'alacritty-v2' as const,
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
      const defaultAgent = useSettingsStore.getState().defaultAgent
      const presets = usePresetsStore.getState().presets
      const preset = presets.find((p) => p.id === defaultAgent)
      if (!preset) return

      const { command, args } = parseCommand(preset.command)

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
  if (agentStatusUnsub) {
    agentStatusUnsub()
    agentStatusUnsub = null
  }
  if (agentHelloUnsub) {
    agentHelloUnsub()
    agentHelloUnsub = null
  }
  if (sessionAddedUnsub) {
    sessionAddedUnsub()
    sessionAddedUnsub = null
  }
  if (sessionRemovedUnsub) {
    sessionRemovedUnsub()
    sessionRemovedUnsub = null
  }
  _liveSessionCwdByKey.clear()
  if (hookUnlisten) {
    hookUnlisten()
    hookUnlisten = null
  }
  // Cancel any pending retry timeouts to prevent ghost launches
  for (const timer of _retryTimeouts) {
    clearTimeout(timer)
  }
  _retryTimeouts.clear()
  _agentStartTimes.clear()
  _launchRetries.clear()
}

// ── #625: host-switch reset of agent pane state ─────────────────────────
//
// `paneStatuses` / `paneProjectMap` / `agents` / `outputTimestamps` (store)
// and `_hookEventAt` / `_agentStartTimes` / `_launchRetries` (module Maps)
// are keyed by pane/terminal IDs that belong to the LOCAL machine's
// workspaces (paneProjectMap maps pane → LOCAL projectId). As store/module
// singletons they survive the `<App key={hostKey}>` remount on a host
// switch, so after connecting to a REMOTE daemon `getProjectStatus` /
// `getAggregateStatus` / `hasActiveAgents` would attribute the local box's
// spinners to the remote — lighting the IconRail active dots and the
// Active Bar `paneStatuses` read against the wrong host.
//
// On a real host CHANGE, wipe all of it: the new host's own polling +
// lifecycle hooks repopulate it for the remote's panes. Pending local
// retry timers are cancelled so a queued local triage can't fire against
// the remote.
export function __resetAgentStateForHostSwitch(): void {
  for (const timer of _retryTimeouts) {
    clearTimeout(timer)
  }
  _retryTimeouts.clear()
  _hookEventAt.clear()
  _agentStartTimes.clear()
  _launchRetries.clear()
  // #688 — the live-session cwd tracking is keyed by the LOCAL host's
  // sessions; wipe it on a host change so the new host's snapshot poll
  // re-seeds `liveSessionCwds` cleanly (and a stale local key can't keep a
  // remote cwd falsely green).
  _liveSessionCwdByKey.clear()
  useActiveAgentsStore.setState({
    agents: new Map(),
    outputTimestamps: new Map(),
    paneStatuses: new Map(),
    paneProjectMap: new Map(),
    backgroundSpawns: [],
    liveSessionCwds: new Set(),
  })
}

onActiveHostChange(() => {
  __resetAgentStateForHostSwitch()
})
