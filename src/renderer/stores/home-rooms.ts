// Home M4 — remote rooms on Home (prd-home-multi-server-client MS24, MS39,
// MS40, MS45, MS55; prd-home-v1 R6–R9, R11).
//
// With "Open agents from other servers here" on (the default on macOS and
// Linux from 0.43.2), a Home row on another server opens THAT server's room
// in Home's main area. The window's server does not change.
// The room is the same components the Agents page mounts (R8/R9), bound to
// a pinned room (`createPinnedRoom`) on the row's server.
//
// Home M5: the room is USABLE (typing, tabs, closes, splits, files, chat
// history, heartbeats — all on its server, through the room allowlist) when
// its server has the layout revision check (`with_revision`, the P0 daemon
// half): the room then saves B's layout with `baseRevision` and merges on a
// 409. An older server keeps M4's view-only room, with a note saying why
// (MS43, plan decision 7). The probe reads B's layout once per build.
//
// This store owns, per window:
//   - the pinned rooms Home has opened, keyed by Home row address;
//   - which one is on screen (`shown`; null = the window's own room);
//   - their tiers (`lib/room-tiers.ts`): hot rooms stay mounted (shown or
//     hidden), so their grids, overlays and workspace socket stay open; a
//     warm room is unmounted (grids close) but keeps its tabs store and its
//     ONE workspace socket (presence on B, tab strip freshness); a cold room
//     is disposed (no sockets) and forgotten — showing it again rebuilds it;
//   - its keep-alive (`lib/room-keep-alive.ts`): `projects/activate` on B
//     through the pool on open, on focus, after an idle hour, and hourly
//     while hot (MS39).
//
// A top-switcher change (prd-home-seamless-0432 Z6/Z7, superseding Q1)
// keeps every pinned room on a server other than the switch's destination:
// its tabs store, workspace socket and carrier, server view, heartbeats,
// keep-alive, tier and `shown`. Only the destination's rooms close (a row
// on the window's server is the window's own room, MS13), and a shown one
// is promoted: the primary room selects the same workspace once the
// destination's list lands. The compare is on Home host keys, so a session
// mint or an id-only re-key of the window's server closes nothing.

import { create, type StoreApi, type UseBoundStore } from 'zustand'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { createPinnedRoom, type PinnedRoom, type PinnedRoomInput, type RoomProjectsStore } from '@/stores/room'
import { roomTiers, type RoomTierChange, type RoomTierManager } from '@/lib/room-tiers'
import { createRoomKeepAlive, type RoomKeepAlive } from '@/lib/room-keep-alive'
import { hostPool } from '@/lib/host-pool-instance'
import { fetchServerProjects, primaryWorkspaceOf } from '@/lib/server-projects'
import { activeHomeHostKey, findWorkspaceForRow } from '@/lib/home-address'
import { requestHostSelect } from '@/lib/home-pending-select'
import { homeRoomVerdict } from '@/lib/home-room-floor'
import { switchWindowToRow, toastOldServerOnce } from '@/lib/home-switch'
import { createStore } from 'zustand/vanilla'
import type { HomeRow } from '@/stores/homes'
import { onActiveHostChange, useConnectHostStore } from '@/stores/connect-host'
import type { KeepAliveResult } from '@/lib/host-pool'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { daemonCliGet } from '@/lib/daemon-cli'
import { onProjectsChanged } from '@/stores/session-events'

export type HomeRoomPhase =
  /** Reading that server's project list. */
  | 'resolving'
  /** The room exists (its tier says how connected it is). */
  | 'open'
  /** The row's agent is not on that server any more. */
  | 'not-found'
  /** The list could not be read (the failure gate shows why). */
  | 'error'
  /** Only ever RETURNED by `open` / `retry`, never stored: the server turned
   *  out to be below the floor once the pool knew its version (Z42). The
   *  entry is gone; the row switched the window if it was on screen. */
  | 'switched'

/** Home M5: what the user can do in an open room. */
export type HomeRoomAccess =
  /** Type, open / close / split tabs, write files … on that server. */
  | 'use'
  /** That server has no layout revision check (an older K2): view only
   *  until it updates (MS43). */
  | 'view-older-server'

export interface HomeRoomEntry {
  /** The Home row address (`handle::host`). */
  address: string
  hostKey: string
  label: string
  phase: HomeRoomPhase
  error: string | null
  room: PinnedRoom | null
  /** Home M5: null until the room is open. */
  access: HomeRoomAccess | null
  /** Bumped on every (re)build, so React remounts a rebuilt room. */
  generation: number
}

interface HomeRoomsState {
  entries: Record<string, HomeRoomEntry>
  /** The row address on screen, or null (the window's own room). */
  shown: string | null
}

export interface HomeRoomsDeps {
  tiers: RoomTierManager
  /** Build the pinned room (tests pass a fake). */
  createRoom(input: PinnedRoomInput): PinnedRoom
  listProjects(scope: ServerScope): Promise<ProjectWithWorkspaces[]>
  /** Make sure the pool knows that server's version before the room is
   *  built: the room's feature gates (`scope.serverSupports`, e.g. B's
   *  tab-order broadcasts) are decided when its socket opens. */
  knowServer(hostKey: string): Promise<void>
  keepAlive(hostKey: string, projectId: string): Promise<KeepAliveResult>
  scopeFor(hostKey: string): ServerScope
  /** Home M5 (MS43): does that server answer the layout read with its
   *  revision (`with_revision`)? Only then may the room save B's layout. */
  layoutRevisionSupported(scope: ServerScope, projectId: string, workspaceId: string): Promise<boolean>
  /** Home M5: that server's `projects_changed` (a worktree created or closed
   *  in the room), so the room's project list follows its server. */
  onProjectsChanged(scope: ServerScope, fn: () => void): () => void
  /** Z5/Z42: the server's version once `knowServer` resolved: null while
   *  the pool has not read its `/boot-status` (the room goes on and fails
   *  on its own), else whether it is below the floor and its version. */
  floorCheck(hostKey: string): { belowFloor: boolean; version: string | null } | null
  /** Z5/Z42: a room's server is below the floor. Switch the window to the
   *  row's server (only when `switchWindow`: the room was on screen) and
   *  say why once per server. */
  belowFloor(row: HomeRow, hostKey: string, version: string | null, switchWindow: boolean): void
  /** Z7/Z29: the window just switched to the shown room's server. Ask the
   *  primary room to select that row's workspace once the new server's
   *  list lands (`requestHostSelect`, keyed by the switcher id). Called
   *  synchronously inside the host-change subscriber pass. */
  promote(row: HomeRow): void
  now(): number
  setInterval(fn: () => void, ms: number): unknown
  clearInterval(handle: unknown): void
}

export interface HomeRooms {
  readonly store: UseBoundStore<StoreApi<HomeRoomsState>>
  /** Open (or show again) the row's room on `hostKey`. Resolves when the
   *  room's layout is loaded, or the row resolved to not-found / error. */
  open(row: HomeRow, hostKey: string): Promise<HomeRoomEntry>
  /** Show the window's own room (a local row, or leaving a remote row). */
  showPrimary(): void
  /** The room was focused (pointer or keyboard inside it). */
  focused(address: string): void
  /** Input in the room (sends the keep-alive after an idle hour). */
  input(address: string): void
  /** Close one room now (dispose, forget). */
  close(address: string): Promise<void>
  /** Close every room (tests). */
  closeAll(): Promise<void>
  /** Z7: the window's server changed from `prevHomeKey` to `nextHomeKey`
   *  (Home host keys). Same key: nothing. Otherwise only the rooms on
   *  `nextHomeKey` close (the shown one is promoted first); every other
   *  room is kept as it is. Resolves when those rooms are disposed. */
  onWindowHostChanged(prevHomeKey: string, nextHomeKey: string): Promise<void>
  /** Retry a room that failed to resolve. */
  retry(address: string): Promise<HomeRoomEntry | null>
  /** Home itself left / came back on screen (another page, Settings): the
   *  shown room's off-screen clock starts / stops. */
  setPageVisible(visible: boolean): void
}

export function createHomeRooms(deps: HomeRoomsDeps): HomeRooms {
  const store = create<HomeRoomsState>(() => ({ entries: {}, shown: null }))
  const keepAlives = new Map<string, RoomKeepAlive>()
  const projectSubs = new Map<string, () => void>()
  const rows = new Map<string, HomeRow>()
  let generation = 0
  /** Is Home on screen? A shown room is only visible (hot, focused) then. */
  let pageVisible = true

  const patch = (address: string, next: Partial<HomeRoomEntry>): HomeRoomEntry => {
    const prev = store.getState().entries[address]
    if (!prev) throw new Error(`home rooms: no entry for ${address}`)
    const entry = { ...prev, ...next }
    store.setState((s) => ({ entries: { ...s.entries, [address]: entry } }))
    return entry
  }

  const addressForRoomKey = (roomKey: string): string | null => {
    for (const e of Object.values(store.getState().entries)) {
      if (e.room?.key === roomKey) return e.address
    }
    return null
  }

  /** Dispose a room and forget its entry. */
  const teardown = async (address: string): Promise<void> => {
    const entry = store.getState().entries[address]
    if (!entry) return
    const ka = keepAlives.get(address)
    keepAlives.delete(address)
    ka?.dispose()
    projectSubs.get(address)?.()
    projectSubs.delete(address)
    store.setState((s) => {
      const entries = { ...s.entries }
      delete entries[address]
      return { entries, shown: s.shown === address ? null : s.shown }
    })
    if (entry.room) {
      deps.tiers.close(entry.room.key)
      await entry.room.dispose()
    }
  }

  deps.tiers.onTierChange((change: RoomTierChange) => {
    const address = addressForRoomKey(change.roomKey)
    if (!address) return
    const ka = keepAlives.get(address)
    if (change.to === 'cold') {
      void teardown(address)
      return
    }
    ka?.setHot(change.to === 'hot')
  })

  const build = async (address: string): Promise<HomeRoomEntry> => {
    const row = rows.get(address)
    if (!row) throw new Error(`home rooms: no row for ${address}`)
    const entry = store.getState().entries[address]
    if (!entry) throw new Error(`home rooms: no entry for ${address}`)
    const scope = deps.scopeFor(entry.hostKey)
    let projects: ProjectWithWorkspaces[]
    try {
      await deps.knowServer(entry.hostKey)
      if (!store.getState().entries[address]) return entry
      // Z42: a row opened before the pool knew this server's version. Below
      // the floor, the room gives way to today's switch (Z5).
      const floor = deps.floorCheck(entry.hostKey)
      if (floor?.belowFloor) {
        const wasShown = store.getState().shown === address
        await teardown(address)
        deps.belowFloor(row, entry.hostKey, floor.version, wasShown)
        return { ...entry, phase: 'switched' }
      }
      projects = await deps.listProjects(scope)
    } catch (err) {
      if (!store.getState().entries[address]) return entry
      return patch(address, { phase: 'error', error: err instanceof Error ? err.message : String(err) })
    }
    if (!store.getState().entries[address]) return entry
    const project = findWorkspaceForRow(projects, row)
    const ws = project ? primaryWorkspaceOf(project) : null
    if (!project || !ws) return patch(address, { phase: 'not-found', error: null })

    let usable: boolean
    try {
      usable = await deps.layoutRevisionSupported(scope, project.id, ws.id)
    } catch (err) {
      if (!store.getState().entries[address]) return entry
      return patch(address, { phase: 'error', error: err instanceof Error ? err.message : String(err) })
    }
    if (!store.getState().entries[address]) return entry
    const access: HomeRoomAccess = usable ? 'use' : 'view-older-server'

    const projectsStore = createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects }))
    projectSubs.get(address)?.()
    projectSubs.set(
      address,
      deps.onProjectsChanged(scope, () => {
        deps.listProjects(scope).then(
          (next) => projectsStore.setState({ projects: next }),
          (err) => console.warn(`[home-rooms] ${entry.hostKey} projects refresh failed:`, err),
        )
      }),
    )
    const room = deps.createRoom({
      scope,
      workspace: { projectId: project.id, workspaceId: ws.id, path: ws.worktreePath ?? project.path },
      projects: projectsStore as RoomProjectsStore,
      // MS39: the room's open/attach ⇒ activate goes to B through the pool.
      activateProject: (pid) => {
        void deps.keepAlive(entry.hostKey, pid)
      },
      readOnly: access !== 'use',
    })
    const ka = createRoomKeepAlive({
      hostKey: entry.hostKey,
      projectId: project.id,
      keepAlive: deps.keepAlive,
      now: deps.now,
      setInterval: deps.setInterval,
      clearInterval: deps.clearInterval,
    })
    keepAlives.set(address, ka)
    generation += 1
    const opened = patch(address, { phase: 'open', error: null, room, access, generation })
    // Register with the tier manager either way; a room the user already
    // moved away from starts its off-screen clock right away.
    deps.tiers.show(room.key)
    if (!(store.getState().shown === address && pageVisible)) deps.tiers.hide(room.key)
    ka.setHot(deps.tiers.tier(room.key) === 'hot')
    await room.tabs.room.open()
    // The room's pinned Chat and Inbox tabs, as the window's own room gets
    // them after a workspace restore (R3). Agent names come from the room's
    // server (`agents/list`). A usable room saves them like B's own window
    // does (with the revision check); a view-only room saves nothing.
    room.tabs.room.ensurePinnedAgentTabForMode(project.agentMode ?? 'off', project.path)
    void ka.opened()
    return opened
  }

  return {
    store,
    async open(row, hostKey) {
      const address = row.address
      rows.set(address, row)
      const prevShown = store.getState().shown
      const prevRoom = prevShown && prevShown !== address ? store.getState().entries[prevShown]?.room : null
      if (prevRoom) deps.tiers.hide(prevRoom.key)
      const existing = store.getState().entries[address]
      store.setState({ shown: address })
      if (existing?.phase === 'open' && existing.room) {
        if (pageVisible) deps.tiers.show(existing.room.key)
        void keepAlives.get(address)?.focused()
        return existing
      }
      if (existing?.phase === 'resolving') return existing
      store.setState((s) => ({
        entries: {
          ...s.entries,
          [address]: {
            address,
            hostKey,
            label: row.label,
            phase: 'resolving',
            error: null,
            room: null,
            access: null,
            generation: existing?.generation ?? 0,
          },
        },
      }))
      return build(address)
    },
    showPrimary() {
      const shown = store.getState().shown
      if (shown === null) return
      const room = store.getState().entries[shown]?.room
      if (room) deps.tiers.hide(room.key)
      store.setState({ shown: null })
    },
    focused(address) {
      void keepAlives.get(address)?.focused()
    },
    input(address) {
      void keepAlives.get(address)?.input()
    },
    close: teardown,
    async closeAll() {
      const addresses = Object.keys(store.getState().entries)
      await Promise.all(addresses.map((a) => teardown(a)))
      store.setState({ shown: null })
    },
    async onWindowHostChanged(prevHomeKey, nextHomeKey) {
      if (prevHomeKey === nextHomeKey) return
      const { entries, shown } = store.getState()
      const onDestination = Object.values(entries).filter((e) => e.hostKey === nextHomeKey)
      if (onDestination.length === 0) return
      // Promote before anything async: the primary room's restore reads
      // the request when the destination's list lands (Z29).
      const shownEntry = shown ? entries[shown] : undefined
      if (shownEntry && shownEntry.hostKey === nextHomeKey) {
        const row = rows.get(shownEntry.address)
        if (row) deps.promote(row)
      }
      // Each room is disposed through its own scope (Z12): its based save
      // flushes to that server, then its sockets close.
      await Promise.all(onDestination.map((e) => teardown(e.address)))
    },
    setPageVisible(visible) {
      pageVisible = visible
      const shown = store.getState().shown
      const room = shown ? store.getState().entries[shown]?.room : null
      if (!room) return
      if (visible) deps.tiers.show(room.key)
      else deps.tiers.hide(room.key)
    },
    async retry(address) {
      const entry = store.getState().entries[address]
      if (!entry || entry.phase === 'open' || entry.phase === 'resolving') return entry ?? null
      patch(address, { phase: 'resolving', error: null })
      return build(address)
    },
  }
}

/** MS43: B answers `workspace-layouts/load?with_revision=1` with
 *  `{layoutJson, revision}` (P0 daemon half, `2a12ec7b`); an older daemon
 *  ignores the flag and answers with the bare layout string (or null). */
export async function layoutRevisionSupported(
  scope: ServerScope,
  projectId: string,
  workspaceId: string,
): Promise<boolean> {
  const res = await daemonCliGet<unknown>(scope, 'workspace-layouts/load', {
    project_id: projectId,
    workspace_id: workspaceId,
    with_revision: '1',
  })
  return (
    res !== null &&
    typeof res === 'object' &&
    typeof (res as { revision?: unknown }).revision === 'number'
  )
}

/** Z7/Z29: hand a Home row to the window's primary room. The request is
 *  keyed by the switcher id of the server the window is on NOW (`'local'`
 *  or `ConnectHost.id`), which is what the projects restore matches
 *  (`takeHostSelect`). */
export function promoteToPrimary(row: HomeRow): void {
  const active = useConnectHostStore.getState().activeHost
  requestHostSelect(active === 'local' ? 'local' : active.id, row)
}

/** This window's Home rooms. */
export const homeRooms: HomeRooms = createHomeRooms({
  tiers: roomTiers,
  createRoom: createPinnedRoom,
  listProjects: fetchServerProjects,
  knowServer: async (hostKey) => {
    if (hostPool.entry(hostKey)?.boot?.version) return
    await hostPool.check(hostKey)
  },
  floorCheck: (hostKey) => {
    const boot = hostPool.entry(hostKey)?.boot ?? null
    if (!boot) return null
    return { belowFloor: homeRoomVerdict(boot) === 'switch', version: boot.version }
  },
  belowFloor: (row, hostKey, version, switchWindow) => {
    toastOldServerOnce(hostKey, version)
    if (switchWindow) switchWindowToRow(row)
  },
  keepAlive: (hostKey, projectId) => hostPool.keepRoomAlive(hostKey, projectId),
  scopeFor: (hostKey) => scopeForHost(hostKey),
  layoutRevisionSupported,
  onProjectsChanged: (scope, fn) => onProjectsChanged(scope, () => fn()),
  promote: promoteToPrimary,
  now: () => Date.now(),
  setInterval: (fn, ms) => setInterval(fn, ms),
  clearInterval: (h) => clearInterval(h as ReturnType<typeof setInterval>),
})

// Z6/Z7 (supersedes Q1): a top-switcher change keeps the rooms. The
// subscriber fires on an `activeHostKey` change AND on a same-host session
// mint, so it compares Home host keys itself (Z29): a mint or a saved host
// re-keyed by id is the same server and closes nothing.
export function installWindowHostSwitch(rooms: Pick<HomeRooms, 'onWindowHostChanged'>): () => void {
  let lastHomeKey = activeHomeHostKey(useConnectHostStore.getState().activeHost)
  return onActiveHostChange(() => {
    const next = activeHomeHostKey(useConnectHostStore.getState().activeHost)
    const prev = lastHomeKey
    lastHomeKey = next
    void rooms.onWindowHostChanged(prev, next)
  })
}
installWindowHostSwitch(homeRooms)

/** React: the Home rooms state. */
export const useHomeRoomsStore = homeRooms.store

/** React: the address of the remote room on screen, or null. */
export function useShownHomeRoom(): string | null {
  return useHomeRoomsStore((s) => s.shown)
}
