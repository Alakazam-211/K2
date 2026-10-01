// Home M4 — remote rooms on Home (prd-home-multi-server-client MS24, MS39,
// MS40, MS45, MS55; prd-home-v1 R6–R9, R11).
//
// With "Remote rooms (preview)" on, a Home row on another server opens THAT
// server's room in Home's main area. The window's server does not change.
// The room is the same components the Agents page mounts (R8/R9), bound to
// a pinned room (`createPinnedRoom`) on the row's server, view-only in M4.
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
// A server switch (Q1) closes every pinned room: the window's server is now
// different, and a row on it opens as the window's own room.

import { create, type StoreApi, type UseBoundStore } from 'zustand'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { createPinnedRoom, type PinnedRoom, type PinnedRoomInput, type RoomProjectsStore } from '@/stores/room'
import { roomTiers, type RoomTierChange, type RoomTierManager } from '@/lib/room-tiers'
import { createRoomKeepAlive, type RoomKeepAlive } from '@/lib/room-keep-alive'
import { hostPool } from '@/lib/host-pool-instance'
import { fetchServerProjects, primaryWorkspaceOf } from '@/lib/server-projects'
import { findWorkspaceForRow } from '@/lib/home-address'
import { createStore } from 'zustand/vanilla'
import type { HomeRow } from '@/stores/homes'
import { onActiveHostChange } from '@/stores/connect-host'
import type { KeepAliveResult } from '@/lib/host-pool'
import type { ProjectWithWorkspaces } from '@/stores/projects'

export type HomeRoomPhase =
  /** Reading that server's project list. */
  | 'resolving'
  /** The room exists (its tier says how connected it is). */
  | 'open'
  /** The row's agent is not on that server any more. */
  | 'not-found'
  /** The list could not be read (the failure gate shows why). */
  | 'error'

export interface HomeRoomEntry {
  /** The Home row address (`handle::host`). */
  address: string
  hostKey: string
  label: string
  phase: HomeRoomPhase
  error: string | null
  room: PinnedRoom | null
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
  keepAlive(hostKey: string, projectId: string): Promise<KeepAliveResult>
  scopeFor(hostKey: string): ServerScope
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
  /** Close every room (server switch, tests). */
  closeAll(): Promise<void>
  /** Retry a room that failed to resolve. */
  retry(address: string): Promise<HomeRoomEntry | null>
  /** Home itself left / came back on screen (another page, Settings): the
   *  shown room's off-screen clock starts / stops. */
  setPageVisible(visible: boolean): void
}

export function createHomeRooms(deps: HomeRoomsDeps): HomeRooms {
  const store = create<HomeRoomsState>(() => ({ entries: {}, shown: null }))
  const keepAlives = new Map<string, RoomKeepAlive>()
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
      projects = await deps.listProjects(scope)
    } catch (err) {
      if (!store.getState().entries[address]) return entry
      return patch(address, { phase: 'error', error: err instanceof Error ? err.message : String(err) })
    }
    if (!store.getState().entries[address]) return entry
    const project = findWorkspaceForRow(projects, row)
    const ws = project ? primaryWorkspaceOf(project) : null
    if (!project || !ws) return patch(address, { phase: 'not-found', error: null })

    const projectsStore = createStore<{ projects: ProjectWithWorkspaces[] }>(() => ({ projects }))
    const room = deps.createRoom({
      scope,
      workspace: { projectId: project.id, workspaceId: ws.id, path: ws.worktreePath ?? project.path },
      projects: projectsStore as RoomProjectsStore,
      // MS39: the room's open/attach ⇒ activate goes to B through the pool.
      activateProject: (pid) => {
        void deps.keepAlive(entry.hostKey, pid)
      },
      readOnly: true,
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
    const opened = patch(address, { phase: 'open', error: null, room, generation })
    // Register with the tier manager either way; a room the user already
    // moved away from starts its off-screen clock right away.
    deps.tiers.show(room.key)
    if (!(store.getState().shown === address && pageVisible)) deps.tiers.hide(room.key)
    ka.setHot(deps.tiers.tier(room.key) === 'hot')
    await room.tabs.room.open()
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

/** This window's Home rooms. */
export const homeRooms: HomeRooms = createHomeRooms({
  tiers: roomTiers,
  createRoom: createPinnedRoom,
  listProjects: fetchServerProjects,
  keepAlive: (hostKey, projectId) => hostPool.keepRoomAlive(hostKey, projectId),
  scopeFor: (hostKey) => scopeForHost(hostKey),
  now: () => Date.now(),
  setInterval: (fn, ms) => setInterval(fn, ms),
  clearInterval: (h) => clearInterval(h as ReturnType<typeof setInterval>),
})

// Q1: a server switch reloads Home and its rooms. A row on the new
// server opens as the window's own room; every pinned room closes.
onActiveHostChange(() => {
  void homeRooms.closeAll()
})

/** React: the Home rooms state. */
export const useHomeRoomsStore = homeRooms.store

/** React: the address of the remote room on screen, or null. */
export function useShownHomeRoom(): string | null {
  return useHomeRoomsStore((s) => s.shown)
}
