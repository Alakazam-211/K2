import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { terminalListRunning } from '@/lib/terminal-daemon'
import { serverSupports } from '@/lib/server-capabilities'
import {
  subscribeToWorkspaceTabEvents,
  type HeartbeatStateChangedEvent,
  type UnsubscribeFn,
} from '@/stores/session-events'

/**
 * Heartbeat sessions store — drives the sidebar Heartbeats panel.
 *
 * Joins three sources for the active workspace:
 *   1. agent_heartbeats rows (active + archived) — via Tauri commands
 *   2. running-agents telemetry — to derive the live indicator
 *   3. CLI session-id history — to know if a row is "Resumable"
 *
 * The panel polls; transitions are not WebSocket-driven yet — see
 * `.k2so/prds/heartbeats-sidebar-audit.md` Phase 2.4 for the lazy
 * subscription rationale (silent autonomous heartbeats avoid the
 * full grid WS bandwidth cost).
 */

export type HeartbeatSessionState = 'live' | 'resumable' | 'scheduled' | 'archived'

export interface HeartbeatRow {
  id: string
  projectId: string
  name: string
  frequency: string
  specJson: string
  wakeupPath: string
  enabled: boolean
  lastFired: string | null
  lastSessionId: string | null
  archivedAt: string | null
  createdAt: number
  /** Daemon-side terminal id of the live PTY currently attached to
   *  this heartbeat, or null when no PTY is alive. Stamped at spawn
   *  time inside smart_launch; cleared on PTY exit (child-exit
   *  observer) or on lazy cleanup when openHeartbeatTab observes the
   *  PTY no longer exists. The TabBar uses this to detect "this open
   *  tab is the live heartbeat session" without depending on
   *  per-tab stamped metadata. See migration 0036 +
   *  `.k2so/prds/heartbeat-active-session-tracking.md`. */
  activeTerminalId?: string | null
}

export interface HeartbeatEntry {
  /** The DB row. */
  row: HeartbeatRow
  /** Derived display state. */
  state: HeartbeatSessionState
  /** Live PTY terminal id when state === 'live'; null otherwise.
   *  Used by the click-to-focus handler in the sidebar. */
  liveTerminalId: string | null
}

interface HeartbeatSessionsState {
  /** Active (non-archived) heartbeats for the currently-loaded project. */
  active: HeartbeatEntry[]
  /** Archived heartbeats — rendered in the sidebar's collapsed
   *  Archived section. */
  archived: HeartbeatEntry[]
  /** projectPath the cached lists belong to; null when never loaded. */
  loadedFor: string | null
  /** True while a refresh is in-flight (suppresses overlapping polls). */
  loading: boolean
  /** Last error from the refresh path. The panel surfaces this directly
   *  instead of silently rendering an empty list. */
  lastError: string | null

  refresh: (projectPath: string) => Promise<void>
  /** Clear cached state (call when no workspace is active). */
  clear: () => void
  /** 0.39.39 (#677.1) — apply a daemon `heartbeat_state_changed` broadcast.
   *  Flips the matching row's live flag without a poll. The event only
   *  carries `live` (no terminal id), so live=true sets state 'live' (and
   *  preserves any known liveTerminalId); live=false re-derives the
   *  idle state (resumable/scheduled) from the row. Matches on
   *  (projectId, heartbeat name). No-op when the row isn't loaded. */
  applyHeartbeatLive: (projectId: string, agent: string, live: boolean) => void
}

/** Idle (non-live) display state for a heartbeat row: resumable when it
 *  has a prior CLI session id, else scheduled. Archived rows are handled
 *  by the caller (they never go live). */
function idleStateForRow(row: HeartbeatRow): HeartbeatSessionState {
  if (row.lastSessionId && row.lastSessionId.length > 0) return 'resumable'
  return 'scheduled'
}

interface RunningAgentInfo {
  terminalId: string
  cwd: string
  command: string | null
}

/**
 * Map a heartbeat row to its display state by joining against running PTY
 * telemetry. The match is approximate — heartbeat-spawned PTYs use a
 * `wake-<agent>-<uuid>` terminal id and don't carry the heartbeat name on
 * the wire today, so we fall back to "any PTY for this agent in this
 * project" as a liveness proxy. Will tighten when the daemon learns to
 * tag PTYs with their owning heartbeat row in P3.x.
 */
function deriveState(
  row: HeartbeatRow,
  agentName: string | null,
  projectPath: string,
  running: RunningAgentInfo[],
): { state: HeartbeatSessionState; liveTerminalId: string | null } {
  if (row.archivedAt) {
    return { state: 'archived', liveTerminalId: null }
  }
  // Liveness proxy: any wake-* PTY whose cwd matches the project root.
  // Tighten in P3.x once heartbeat→PTY linking is explicit.
  const live = agentName
    ? running.find(
        (r) =>
          r.terminalId.startsWith(`wake-${agentName}-`) &&
          r.cwd.startsWith(projectPath),
      )
    : null
  if (live) {
    return { state: 'live', liveTerminalId: live.terminalId }
  }
  if (row.lastSessionId && row.lastSessionId.length > 0) {
    return { state: 'resumable', liveTerminalId: null }
  }
  return { state: 'scheduled', liveTerminalId: null }
}

/**
 * Pick the workspace's primary agent. Same resolution the
 * WorkspacePanel header uses so the live-state liveness check
 * keys on the right agent. Without this, a `custom` workspace
 * that also keeps a `k2so-agent` template alongside its custom
 * agent would resolve to the alphabetically-first dir
 * (`k2so-agent`) and miss the actual primary's wake- PTYs.
 */
export async function resolvePrimaryAgent(projectPath: string): Promise<string | null> {
  try {
    const list = await invoke<Array<{ name: string; isManager?: boolean; agentType?: string }>>(
      'k2so_agents_list',
      { projectPath },
    )
    if (list.length === 0) return null
    // Read agentMode from the projects store (no IPC needed; already
    // synced). Fall back to alphabetical first only when the store
    // doesn't have the project — defensive, shouldn't happen in normal
    // flows since refresh is only called for active workspaces.
    const { useProjectsStore } = await import('@/stores/projects')
    const project = useProjectsStore.getState().projects.find((p) => p.path === projectPath)
    const agentMode = project?.agentMode ?? 'off'
    if (agentMode === 'manager' || agentMode === 'coordinator' || agentMode === 'pod') {
      return (
        list.find((a) => a.isManager || a.agentType === 'manager' || a.agentType === 'coordinator')?.name
        ?? list[0].name
      )
    }
    if (agentMode === 'custom') {
      return list.find((a) => a.agentType === 'custom')?.name ?? list[0].name
    }
    if (agentMode === 'agent') {
      return list.find((a) => a.agentType === 'k2so')?.name ?? list[0].name
    }
    return list[0].name
  } catch {
    return null
  }
}

export const useHeartbeatSessionsStore = create<HeartbeatSessionsState>((set, get) => ({
  active: [],
  archived: [],
  loadedFor: null,
  loading: false,
  lastError: null,

  refresh: async (projectPath: string): Promise<void> => {
    if (get().loading) return
    set({ loading: true, lastError: null })

    try {
      const [activeRows, archivedRows, running, agentName] = await Promise.all([
        invoke<HeartbeatRow[]>('k2so_heartbeat_list', { projectPath }),
        invoke<HeartbeatRow[]>('k2so_heartbeat_list_archived', { projectPath }),
        terminalListRunning().catch((): RunningAgentInfo[] => []),
        resolvePrimaryAgent(projectPath),
      ])

      const active: HeartbeatEntry[] = activeRows.map((row) => ({
        row,
        ...deriveState(row, agentName, projectPath, running),
      }))
      const archived: HeartbeatEntry[] = archivedRows.map((row) => ({
        row,
        state: 'archived' as const,
        liveTerminalId: null,
      }))

      set({ active, archived, loadedFor: projectPath, loading: false })
    } catch (err) {
      const msg = String(err)
      console.error('[heartbeat-sessions] refresh failed:', msg)
      set({ loading: false, lastError: msg })
    }
  },

  clear: () => {
    set({ active: [], archived: [], loadedFor: null, lastError: null })
  },

  applyHeartbeatLive: (projectId: string, agent: string, live: boolean): void => {
    set((state) => {
      let changed = false
      const active = state.active.map((entry) => {
        const row = entry.row
        if (row.projectId !== projectId || row.name !== agent) return entry
        if (row.archivedAt) return entry // archived rows never go live
        const nextState: HeartbeatSessionState = live ? 'live' : idleStateForRow(row)
        // live=true keeps any known liveTerminalId (the event omits it, so
        // we don't have a fresher one); live=false clears it.
        const nextTerminalId = live ? entry.liveTerminalId : null
        if (entry.state === nextState && entry.liveTerminalId === nextTerminalId) return entry
        changed = true
        return { ...entry, state: nextState, liveTerminalId: nextTerminalId }
      })
      return changed ? { active } : {}
    })
  },
}))

// 0.39.39 (#677.1) — push-primary heartbeat live-dot. The per-renderer
// derive-vs-`terminal/list-running` join (`deriveState`) only reflects
// liveness at refresh time and never updates a headless/remote-driven
// wake. Against a daemon that emits the Wave B broadcasts we subscribe to
// `heartbeat_state_changed` and flip the row's live flag directly; the
// `refresh()` derivation remains the initial snapshot + the fallback when
// the daemon doesn't support the broadcast. The subscription is keyed on
// the loaded workspace path and swapped when the workspace changes.
let hbEventsUnsub: UnsubscribeFn | null = null
let hbSubscribedPath: string | null = null

/** (Re)subscribe the heartbeat live-dot stream to `projectPath`. No-op when
 *  already subscribed to that path or when the daemon lacks the broadcast
 *  capability (the `refresh()` derivation covers display). Pass `null` to
 *  tear the subscription down (no workspace active). */
export function subscribeHeartbeatLive(projectPath: string | null): void {
  if (!serverSupports('daemon-broadcasts')) {
    // Unsupported daemon — drop any stale subscription and rely on the
    // refresh-time derivation fallback.
    if (hbEventsUnsub) {
      hbEventsUnsub()
      hbEventsUnsub = null
    }
    hbSubscribedPath = null
    return
  }
  if (projectPath === hbSubscribedPath) return
  if (hbEventsUnsub) {
    hbEventsUnsub()
    hbEventsUnsub = null
  }
  hbSubscribedPath = projectPath
  if (!projectPath) return
  hbEventsUnsub = subscribeToWorkspaceTabEvents(projectPath, {
    onHeartbeatStateChanged: (e: HeartbeatStateChangedEvent) => {
      // `project` is the project_id; the live-event's `project` is the
      // authoritative key (workspacePath may be empty on the spawn path).
      useHeartbeatSessionsStore
        .getState()
        .applyHeartbeatLive(e.project, e.agent, e.live)
    },
    // Re-snapshot on (re)connect to backfill liveness missed during a drop.
    onHello: () => {
      const loadedFor = useHeartbeatSessionsStore.getState().loadedFor
      if (loadedFor) void useHeartbeatSessionsStore.getState().refresh(loadedFor)
    },
  })
}

/** Tear down the heartbeat live-dot subscription (call when no workspace is
 *  active or on host switch). Idempotent. */
export function unsubscribeHeartbeatLive(): void {
  if (hbEventsUnsub) {
    hbEventsUnsub()
    hbEventsUnsub = null
  }
  hbSubscribedPath = null
}
