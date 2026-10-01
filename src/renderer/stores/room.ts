// Rooms (Home M3, prd-home-multi-server-client MS2, MS3, MS14, MS15, MS21,
// MS67, MS68).
//
// A room is one workspace shown on one server: `(hostKey, projectId,
// workspaceId)`. Room components (the tab strip, panes, terminals, drawers)
// read EVERYTHING server-bound from their room — its tabs store, its scope,
// its project list, its activity sink — never from the window's globals.
// `RoomProvider` (components/Room/RoomContext.tsx) hands the room down; a
// room component with no provider throws (MS2). Window-level code (menus,
// shortcuts, the assistant) acts on the FOCUSED room (`stores/window-room.ts`).
//
//   - `primaryRoom()` is the window's own room. It wraps today's singletons:
//     `useTabsStore`, the projects store, the active-agents store. Agents,
//     Home on the connected server, and Focus windows render it, so they
//     behave exactly as before M3.
//   - `createPinnedRoom()` builds a room for one workspace on one scope, with
//     its own tabs store, its own project list and its own activity slice.
//     M3 makes it possible (tests build one); M4 opens pinned rooms in Home.

import { create, type StoreApi } from 'zustand'
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { playCompletionSound } from '@/lib/completion-sound'
import { primaryScope, scopedKey, type ServerScope } from '@/kessel/server-scope'
import { createTabsStore, useTabsStore, type TabsStore, type TabsRoomWorkspace, type ProjectPathEntry } from '@/stores/tabs'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import type { PaneStatus } from '@/stores/active-agents'
import { createHeartbeatSessionsStore, useHeartbeatSessionsStore, type HeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
import { acquireServerView, type ActiveViewStore, type PresenceViewStore } from '@/stores/server-view'
import { PRIMARY_ACTIVE_SET, PRIMARY_PRESENCE } from '@/stores/primary-room-sources'
import { remoteRoomScope, viewOnlyScope } from '@/kessel/server-scope'
import { onSessionActivityChanged, type SessionActivityChangedEvent } from '@/stores/session-events'

/** Where a terminal pane reports what its agent is doing (MS68). The pane
 *  never reaches for a global store: the primary room's sink is the window's
 *  active-agents store (Active bar, tab dots, chime); a pinned room's sink is
 *  its own slice, and its chime reads ITS server's project record. */
export interface RoomActivitySink {
  recordOutput(terminalId: string): void
  recordTitleActivity(terminalId: string, isWorking: boolean): void
  recordTitlePermission(terminalId: string, active: boolean): void
  markSeen(terminalId: string): void
  bindPaneAgentName(agentName: string, terminalId: string): void
  /** Bind a pane to the project it belongs to (the chime and Active-bar
   *  marks read it). */
  bindPaneProject(terminalId: string, projectId: string): void
}

/** A room's project list as a read-only store (MS3: a path is only an
 *  identity inside the server it came from). */
export type RoomProjectsStore = Pick<
  StoreApi<{ projects: ProjectWithWorkspaces[] }>,
  'getState' | 'getInitialState' | 'subscribe'
>

export interface Room {
  /** Stable instance identity: `primary`, or `<hostKey>|<projectId>:<workspaceId>`. */
  readonly key: string
  readonly isPrimary: boolean
  /** The server every request, socket and save of this room goes to. */
  readonly scope: ServerScope
  /** This room's tabs store instance. */
  readonly tabs: TabsStore
  /** This room's own project list (its server's `projects/list`). */
  readonly projects: RoomProjectsStore
  /** MS68 — where its terminal panes report activity. */
  readonly activity: RoomActivitySink
  /** What its tab dots read (the same activity, as state). */
  readonly activityView: RoomActivityViewStore
  /** MS67 — may room code call THIS computer's Tauri commands
   *  (`projects_open_in_*`, `k2so_*`, `read_worktree_file`, local events)?
   *  The primary room keeps today's behaviour (true) until M4 decides the
   *  remote-window case; a pinned room only when its scope is `local`. */
  readonly localCommands: boolean
  /** Home M4 — a view-only (preview) room: it shows its server's tabs,
   *  drawers and terminals and changes nothing there (no typing, no layout
   *  save, attach-only terminals, no file writes). Its `scope` is the
   *  view-only twin, so the request layer refuses writes too. */
  readonly readOnly: boolean
  /** Its server's presence roster (R7, MS14 per server). */
  readonly presence: PresenceViewStore
  /** Its server's canonical Active set (MS14 per server). */
  readonly activeSet: ActiveViewStore
  /** Its heartbeat rows (MS14 per room). */
  readonly heartbeats: HeartbeatSessionsStore
  /** The project the room shows (primary: the window's selected project). */
  activeProjectId(): string | null
  /** `<hostKey>|<projectId>:<workspaceId>` of what the room shows now (MS17),
   *  or null before a workspace is open. */
  roomId(): string | null
  /** The workspace cwd window-level actions open new tabs in. */
  cwd(): string
}

/** Home M5: the S5 mode (`set_mode`) a terminal pane's socket sends in its
 *  room. The primary room keeps the window's eye/pencil mode (null). A
 *  pinned room's comes from the room: view-only → viewer; usable → claimer,
 *  since every login role left on its server (Owner, Admin, Member) may type
 *  — the Viewer role is gone (RV5) and the daemon's claim gate decides. */
export function paneRoomMode(room: Pick<Room, 'isPrimary' | 'readOnly'>): 'viewer' | 'claimer' | null {
  if (room.isPrimary) return null
  return room.readOnly ? 'viewer' : 'claimer'
}

// ── Lookups inside one room (MS3) ────────────────────────────────────────

/** The project in THIS room's list whose path is `path`. Never another
 *  server's: a pinned room's list is its own server's. */
export function roomProjectForPath(room: Room, path: string): ProjectWithWorkspaces | null {
  return room.projects.getState().projects.find((p) => p.path === path) ?? null
}

/** Like `roomProjectForPath`, but a worktree workspace's path also matches
 *  its project. */
export function roomProjectForCwd(room: Room, cwd: string): ProjectWithWorkspaces | null {
  return (
    room.projects
      .getState()
      .projects.find((p) => p.path === cwd || p.workspaces.some((w) => w.worktreePath === cwd)) ?? null
  )
}

/** The room's currently shown project record, from its own list. */
export function roomActiveProject(room: Room): ProjectWithWorkspaces | null {
  const id = room.activeProjectId()
  if (!id) return null
  return room.projects.getState().projects.find((p) => p.id === id) ?? null
}

function roomIdFor(scope: ServerScope, projectId: string | null, workspaceId: string | null): string | null {
  if (!projectId || !workspaceId) return null
  return scopedKey(scope, `${projectId}:${workspaceId}`)
}

// ── The primary room ─────────────────────────────────────────────────────
//
// The projects store and the active-agents store register themselves here
// at module load (they import this module; this module never imports them,
// so a component that only needs the primary room does not pull in their
// whole graph). Using the primary room before they are loaded throws.

/** The window's projects store, as the primary room reads it. */
export type PrimaryProjectsStore = Pick<
  StoreApi<{
    projects: ProjectWithWorkspaces[]
    activeProjectId: string | null
    activeWorkspaceId: string | null
  }>,
  'getState' | 'getInitialState' | 'subscribe'
>

let primaryProjects: PrimaryProjectsStore | null = null
let primaryActivity: RoomActivitySink | null = null
let primaryActivityView: RoomActivityViewStore | null = null

/** projects.ts registers the window's projects store. */
export function registerPrimaryRoomProjects(store: PrimaryProjectsStore): void {
  primaryProjects = store
}

/** active-agents.ts registers the window's activity sink (MS68) and the
 *  store its tab dots read. */
export function registerPrimaryRoomActivity(sink: RoomActivitySink, view: RoomActivityViewStore): void {
  primaryActivity = sink
  primaryActivityView = view
}

function requirePrimaryProjects(): PrimaryProjectsStore {
  if (!primaryProjects) throw new Error('primary room: the projects store is not loaded (registerPrimaryRoomProjects)')
  return primaryProjects
}

function requirePrimaryActivity(): RoomActivitySink {
  if (!primaryActivity) throw new Error('primary room: the active-agents store is not loaded (registerPrimaryRoomActivity)')
  return primaryActivity
}

/** cwd File-menu actions use in the primary room: active workspace, else
 *  project path, else '~' (unchanged from before M3). */
function primaryCwd(): string {
  const ps = requirePrimaryProjects().getState()
  const proj = ps.projects.find((p) => p.id === ps.activeProjectId)
  const ws = proj?.workspaces?.find((w) => w.id === ps.activeWorkspaceId)
  return ws?.worktreePath ?? proj?.path ?? '~'
}

const PRIMARY_PROJECTS: RoomProjectsStore = {
  getState: () => requirePrimaryProjects().getState(),
  getInitialState: () => requirePrimaryProjects().getInitialState(),
  subscribe: (listener) => requirePrimaryProjects().subscribe(listener),
}

const PRIMARY_ACTIVITY_VIEW: RoomActivityViewStore = {
  getState: () => requirePrimaryActivityView().getState(),
  getInitialState: () => requirePrimaryActivityView().getInitialState(),
  subscribe: (listener) => requirePrimaryActivityView().subscribe(listener),
}

function requirePrimaryActivityView(): RoomActivityViewStore {
  if (!primaryActivityView) throw new Error('primary room: the active-agents store is not loaded (registerPrimaryRoomActivity)')
  return primaryActivityView
}

const PRIMARY_ACTIVITY: RoomActivitySink = {
  recordOutput: (id) => requirePrimaryActivity().recordOutput(id),
  recordTitleActivity: (id, working) => requirePrimaryActivity().recordTitleActivity(id, working),
  recordTitlePermission: (id, active) => requirePrimaryActivity().recordTitlePermission(id, active),
  markSeen: (id) => requirePrimaryActivity().markSeen(id),
  bindPaneAgentName: (agentName, id) => requirePrimaryActivity().bindPaneAgentName(agentName, id),
  bindPaneProject: (id, projectId) => requirePrimaryActivity().bindPaneProject(id, projectId),
}

let primary: Room | null = null

/** The window's own room: today's tabs, projects and active-agents stores,
 *  on the window's server (`primaryScope()`, resolved at call time). */
export function primaryRoom(): Room {
  if (primary) return primary
  const scope = primaryScope()
  primary = {
    key: 'primary',
    isPrimary: true,
    scope,
    tabs: useTabsStore,
    projects: PRIMARY_PROJECTS,
    activity: PRIMARY_ACTIVITY,
    activityView: PRIMARY_ACTIVITY_VIEW,
    // Home M4 (MS57): a window connected to a remote server shows that
    // server's paths, so this computer's commands are off there.
    get localCommands(): boolean {
      return !primaryScope().isRemote
    },
    readOnly: false,
    presence: PRIMARY_PRESENCE,
    activeSet: PRIMARY_ACTIVE_SET,
    heartbeats: useHeartbeatSessionsStore,
    activeProjectId: () => requirePrimaryProjects().getState().activeProjectId,
    roomId: () => {
      const s = useTabsStore.getState()
      return roomIdFor(scope, s.activeProjectId, s.activeWorkspaceId)
    },
    cwd: primaryCwd,
  }
  return primary
}

// ── Pinned rooms ─────────────────────────────────────────────────────────

/** What a room's tab dots read (MS14 "per-room slice", MS21): the
 *  primary room reads the window's active-agents store (same fields); a
 *  pinned room its own slice. */
export interface RoomActivityView {
  /** terminal id → what its agent is doing (client-side detection). */
  paneStatuses: Map<string, PaneStatus>
  /** terminal id → the daemon's word on it (empty in a pinned room). */
  daemonPaneStatuses: Map<string, PaneStatus>
  /** terminal id → when it finished while unseen. */
  unseenDone: Map<string, number>
}

export type RoomActivityViewStore = Pick<StoreApi<RoomActivityView>, 'getState' | 'getInitialState' | 'subscribe'>

/** A pinned room's own activity slice (MS14 "per-room slice", MS68). */
export interface RoomActivityState extends RoomActivityView {
  /** daemon agent name → terminal id. */
  aliases: Map<string, string>
}

export type RoomActivityStore = StoreApi<RoomActivityState> &
  RoomActivitySink & {
    /** Home M5 tab dots: the room's server's own word on a session
     *  (`session_activity_changed`, visibility-independent). */
    applyDaemonActivity(e: Pick<SessionActivityChangedEvent, 'agentName' | 'paneGroupId' | 'status'>): void
  }

/** `/w/app` is under `/w`; `/w-other` is not (the daemon's own rule,
 *  `activity_events_ws.rs` boundary). */
export function pathUnderRoot(path: string, root: string): boolean {
  const norm = (p: string): string => p.replace(/\\/g, '/').replace(/\/+$/, '')
  const a = norm(path)
  const r = norm(root)
  return r.length > 0 && (a === r || a.startsWith(`${r}/`))
}

/** Build a pinned room's activity slice. A working → idle transition marks
 *  the pane unseen-done and chimes with THIS room's project record (MS21):
 *  a B project id is never looked up in A's list. */
export function createRoomActivity(
  projects: RoomProjectsStore,
  projectId: string,
): RoomActivityStore {
  const store = create<RoomActivityState>(() => ({
    paneStatuses: new Map(),
    daemonPaneStatuses: new Map(),
    unseenDone: new Map(),
    aliases: new Map(),
  }))
  const setStatus = (id: string, status: PaneStatus): void => {
    const next = new Map(store.getState().paneStatuses)
    next.set(id, status)
    store.setState({ paneStatuses: next })
  }
  const sink: RoomActivitySink = {
    recordOutput: () => {},
    recordTitleActivity: (id, working) => {
      const prev = store.getState().paneStatuses.get(id)
      if (prev === 'permission') return
      if (working) {
        if (prev !== 'working') setStatus(id, 'working')
        return
      }
      if (prev !== 'working') return
      setStatus(id, 'idle')
      const unseen = new Map(store.getState().unseenDone)
      unseen.set(id, Date.now())
      store.setState({ unseenDone: unseen })
      playCompletionSound(projectId, projects.getState().projects)
    },
    recordTitlePermission: (id, active) => {
      const prev = store.getState().paneStatuses.get(id)
      if (active) setStatus(id, 'permission')
      else if (prev === 'permission') setStatus(id, 'idle')
    },
    markSeen: (id) => {
      if (!store.getState().unseenDone.has(id)) return
      const unseen = new Map(store.getState().unseenDone)
      unseen.delete(id)
      store.setState({ unseenDone: unseen })
    },
    bindPaneAgentName: (agentName, id) => {
      const next = new Map(store.getState().aliases)
      next.set(agentName, id)
      store.setState({ aliases: next })
    },
    // A pinned room holds one workspace: every pane is that project's.
    bindPaneProject: () => {},
  }
  const applyDaemonActivity: RoomActivityStore['applyDaemonActivity'] = (e) => {
    const st = store.getState()
    const paneId = e.paneGroupId ?? st.aliases.get(e.agentName) ?? e.agentName
    const prev = st.daemonPaneStatuses.get(paneId)
    if (prev === e.status) return
    const next = new Map(st.daemonPaneStatuses)
    next.set(paneId, e.status)
    store.setState({ daemonPaneStatuses: next })
    // A finished session whose pane is not reporting here (another tab, a
    // hidden pane): mark it unseen-done and chime with THIS room's project.
    // A mounted pane's own title feed already did both (no double chime).
    if (e.status !== 'idle' || (prev !== 'working' && prev !== 'permission')) return
    if (st.paneStatuses.has(paneId)) return
    const unseen = new Map(store.getState().unseenDone)
    unseen.set(paneId, Date.now())
    store.setState({ unseenDone: unseen })
    playCompletionSound(projectId, projects.getState().projects)
  }
  return Object.assign(store, sink, { applyDaemonActivity })
}

export interface PinnedRoomInput {
  scope: ServerScope
  workspace: TabsRoomWorkspace
  /** That server's project list (M4: from its `projects/list`). */
  projects: RoomProjectsStore
  /** That server's open/attach ⇒ activate gesture (M4: the pool's
   *  per-server activate, MS39). */
  activateProject: (projectId: string) => void
  /** That server's agent presets, read-only (MS14). */
  presets?: () => { presets: any[] } | null
  /** Home M4: open it view-only (the M4 preview). Default false. */
  readOnly?: boolean
}

/** A pinned room plus its teardown. */
export interface PinnedRoom extends Room {
  /** Flush (a view-only room saves nothing), close its sockets, drop its
   *  server-view reference. Idempotent. */
  dispose(): Promise<void>
}

function pathIndex(projects: RoomProjectsStore): () => ProjectPathEntry[] {
  return () =>
    projects.getState().projects.map((p) => {
      const sorted = [...(p.workspaces ?? [])].sort((a, b) => a.tabOrder - b.tabOrder)
      return {
        id: p.id,
        path: p.path,
        primaryWorkspaceId: sorted[0]?.id ?? null,
        hideApiSessions: (p.hideApiSessions ?? 0) === 1,
      }
    })
}

/** A room for one workspace on one server. Its tabs store saves, closes and
 *  subscribes on `scope` only. The caller opens it (`room.tabs.room.open()`)
 *  and disposes it (`room.tabs.room.dispose()`). */
export function createPinnedRoom(input: PinnedRoomInput): PinnedRoom {
  const { workspace, projects } = input
  const readOnly = input.readOnly === true
  if (input.scope.isPrimary) throw new Error('createPinnedRoom: a pinned room needs a pinned scope (scopeForHost)')
  // Home M5: a usable room writes to its server through the room allowlist
  // (`remoteRoomScope`, `kessel/room-writes.ts`); a view-only room sends
  // only the keep-alive (`viewOnlyScope`).
  const scope = readOnly ? viewOnlyScope(input.scope) : remoteRoomScope(input.scope)
  const localCommands = scope.hostKey === LOCAL_HOME_HOST
  const heartbeats = createHeartbeatSessionsStore({ scope, localCommands })
  const serverView = acquireServerView(scope)
  const tabs = createTabsStore({
    scope,
    workspace,
    localCommands,
    deps: {
      projectsPathIndex: pathIndex(projects),
      activeProjectId: () => workspace.projectId,
      activateProject: input.activateProject,
      projectDefaultAgent: (projectId) =>
        projects.getState().projects.find((p) => p.id === projectId)?.defaultAgent ?? undefined,
      presets: input.presets ?? (() => null),
      heartbeatEntries: () => heartbeats.getState().active,
    },
  })
  const activity = createRoomActivity(projects, workspace.projectId)
  // Home M5 tab dots: the server's own activity for this workspace's
  // sessions. It rides the room's workspace socket (the app-bus carrier).
  const activityUnsub = onSessionActivityChanged(scope, (e) => {
    if (!pathUnderRoot(e.workspacePath, workspace.path)) return
    activity.applyDaemonActivity(e)
  })
  let disposed: Promise<void> | null = null
  return {
    key: scopedKey(scope, `${workspace.projectId}:${workspace.workspaceId}`),
    isPrimary: false,
    scope,
    tabs,
    projects,
    activity,
    activityView: activity,
    localCommands,
    readOnly,
    presence: serverView.view.presence,
    activeSet: serverView.view.active,
    heartbeats,
    dispose: () => {
      if (!disposed) {
        disposed = (async () => {
          activityUnsub()
          heartbeats.unsubscribeLive()
          try {
            await tabs.room.dispose()
          } finally {
            serverView.release()
          }
        })()
      }
      return disposed
    },
    activeProjectId: () => workspace.projectId,
    roomId: () => roomIdFor(scope, workspace.projectId, workspace.workspaceId),
    cwd: () => workspace.path,
  }
}

/** Test seam: forget the cached primary room (its stores are module
 *  singletons and stay). */
export function __resetPrimaryRoomForTests(): void {
  primary = null
}
