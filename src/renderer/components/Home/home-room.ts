// Home on the Agents shell — which room is showing, and the hooks that keep
// Home rows current (prd-home-v1 H2 + §6, vs-live H11–H18).
//
// Home is the Agents page with the Home roster in the sidebar. The room in
// the main area is the window's ordinary active workspace
// (`projects.activeProjectId`) — no second selection is stored. Home shows
// that room only when the active workspace is a row of the selected Home
// on the connected server; otherwise it shows the Agents empty state.
//
// Nothing here resets on a server switch: the Homes live in `stores/homes`
// (never subscribed to `onActiveHostChange`) and the page stays `home`.
// Row status comes from the connection pool (Home M2, `lib/host-pool.ts`).
// It checks each row's server every 30 s (offline: 5 s, 15 s, then 30 s),
// only while Home is on screen and the window is visible
// (`HomeShellEffects` is mounted only then). The connected server gets the
// public `/boot-status` read only, for its `instanceId` (MS81).

import { useCallback, useEffect, useMemo, useSyncExternalStore } from 'react'
import { create, useStore } from 'zustand'
import { useHomesStore, selectedHome, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { usePageViewStore } from '@/stores/page-view'
import { isWebClient } from '@/lib/is-web'
import {
  LOCAL_HOME_HOST,
  activeHomeHostKey,
  findWorkspaceForRow,
  homeAddress,
  parseHomeAddress,
  savedHostForKey,
  workspaceHandle,
} from '@/lib/home-address'
import {
  HOME_POLL_MS,
  resolveRowStatus,
  roomRowActivity,
  type RoomRowActivity,
  type RowStatus,
} from '@/lib/home-status'
import { mergePaneStatus } from '@/stores/active-agents'
import type { Room } from '@/stores/room'
import { nextCheckDelayMs, sameServerPairs } from '@/lib/host-pool'
import { hostPool } from '@/lib/host-pool-instance'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useZenShown } from '@/lib/zen/zen-view'

/** True when Home is the page and the window's active workspace is a row
 *  of the selected Home on the connected server — the room is shown. */
export function useHomeRoomSelected(): boolean {
  const onHome = usePageViewStore((s) => s.page === 'home')
  const home = useHomesStore(selectedHome)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const projects = useProjectsStore((s) => s.projects)
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  // Home M4: a remote room on screen hides the window's own room.
  const remoteShown = useHomeRoomsStore((s) => s.shown !== null)
  return useMemo(() => {
    if (!onHome || !activeProjectId || remoteShown) return false
    return home.rows.some((row) => {
      const p = parseHomeAddress(row.address)
      if (!p || p.host !== connectedKey) return false
      return findWorkspaceForRow(projects, row)?.id === activeProjectId
    })
  }, [onHome, home.rows, activeProjectId, projects, connectedKey, remoteShown])
}

/** Open state of the Add Agent picker. The collapsed rail's + opens it
 *  after expanding the sidebar, so it is not component-local. */
export const useHomeAddPickerStore = create<{ open: boolean; setOpen: (open: boolean) => void }>((set) => ({
  open: false,
  setOpen: (open) => set({ open }),
}))

/** Where a row lives, for the row's machine chip. `null` = the connected
 *  server (Agents shows no place for those either). */
export function rowPlace(hostKey: string, connectedKey: string, hosts: ReturnType<typeof useConnectHostStore.getState>['hosts']): string | null {
  if (hostKey === connectedKey) return null
  if (hostKey === LOCAL_HOME_HOST) return 'This computer'
  const saved = savedHostForKey(hosts, hostKey)
  return saved ? saved.label : hostKey || 'unknown server'
}

/** Row host keys of the selected Home, in row order, without repeats. */
function homeHostKeys(home: Home): string[] {
  const keys: string[] = []
  for (const r of home.rows) {
    const p = parseHomeAddress(r.address)
    if (p && !keys.includes(p.host)) keys.push(p.host)
  }
  return keys
}

/** MS81 / answer Q2(a): the label of an earlier row's server that answers
 *  with the same `instanceId` as `hostKey`, or null. */
function useSameServerAs(hostKey: string): string | null {
  const home = useHomesStore(selectedHome)
  const hosts = useConnectHostStore((s) => s.hosts)
  const entries = useStore(hostPool.store, (s) => s.entries)
  return useMemo(() => {
    const earlier = sameServerPairs(homeHostKeys(home), entries)[hostKey]
    if (!earlier) return null
    if (earlier === LOCAL_HOME_HOST) return 'This computer'
    const saved = savedHostForKey(hosts, earlier)
    return saved ? saved.label || saved.hostname : earlier
  }, [home, entries, hostKey, hosts])
}

/** Status of a row that does not paint as a live Agents row: a row on
 *  another server (live / starting / offline / sign in / no access), or a
 *  connected-server row whose workspace is gone or still loading. */
export function useRowStatus(row: HomeRow): { status: RowStatus; place: string | null; onConnected: boolean } {
  const parsed = parseHomeAddress(row.address)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  const hostKey = parsed?.host ?? ''
  const entry = useStore(hostPool.store, (s) => s.entries[hostKey])
  const sameServerAs = useSameServerAs(hostKey)
  const room = useHomeRoomsStore((s) => s.entries[row.address]?.room ?? null)
  const roomActivity = useRoomRowActivity(room)
  return computeRowStatus(row, { activeHost, hosts, connectionStatus, projects, entry, sameServerAs, roomActivity })
}

/** Home 0.43.2 (Z23): an open room's activity, live from its own slice
 *  (no poll); null when no room is open for the row. */
function useRoomRowActivity(room: Pick<Room, 'activityView'> | null): RoomRowActivity | null {
  const subscribe = useCallback(
    (onChange: () => void) => (room ? room.activityView.subscribe(onChange) : () => {}),
    [room],
  )
  const read = useCallback(
    () => (room ? roomRowActivity(room.activityView.getState(), mergePaneStatus) : null),
    [room],
  )
  return useSyncExternalStore(subscribe, read, read)
}

export type RowStatusInputs = {
  activeHost: ReturnType<typeof useConnectHostStore.getState>['activeHost']
  hosts: ReturnType<typeof useConnectHostStore.getState>['hosts']
  connectionStatus: ReturnType<typeof useConnectHostStore.getState>['connectionStatus']
  projects: ReturnType<typeof useProjectsStore.getState>['projects']
  entry: Extract<Parameters<typeof resolveRowStatus>[0], { where: 'other' }>['entry']
  sameServerAs: string | null
  /** An open room's live activity for the row (Z23), or null. */
  roomActivity?: RoomRowActivity | null
}

/** The status a row paints, from plain store values (shared by the row's
 *  hook and the Cmd+1–9 shortcut). */
export function computeRowStatus(
  row: HomeRow,
  { activeHost, hosts, connectionStatus, projects, entry, sameServerAs, roomActivity }: RowStatusInputs,
): { status: RowStatus; place: string | null; onConnected: boolean } {
  const parsed = parseHomeAddress(row.address)
  const hostKey = parsed?.host ?? ''
  const connectedKey = activeHomeHostKey(activeHost)
  const onConnected = hostKey === connectedKey
  const place = rowPlace(hostKey, connectedKey, hosts)

  if (!parsed) return { status: { kind: 'not-found', label: 'Not found', people: [] }, place, onConnected }
  if (onConnected) {
    if (connectionStatus !== 'connected') {
      return { status: { kind: 'checking', label: 'Checking…', people: [] }, place, onConnected }
    }
    const ws = findWorkspaceForRow(projects, row)
    return {
      status: ws
        ? { kind: 'idle', label: 'Idle', people: [] }
        : { kind: 'not-found', label: 'Not found', people: [] },
      place,
      onConnected,
    }
  }
  const isLocal = hostKey === LOCAL_HOME_HOST
  const saved = isLocal ? null : savedHostForKey(hosts, hostKey)
  return {
    status: resolveRowStatus({
      where: 'other',
      row,
      saved: isLocal ? !isWebClient() : saved !== null,
      hasLogin: isLocal ? true : (saved?.token.length ?? 0) > 0,
      entry,
      self: isLocal ? 'owner' : saved?.username || 'owner',
      sameServerAs,
      serverLabel: place ?? hostKey,
      roomActivity: roomActivity ?? null,
    }),
    place,
    onConnected,
  }
}

/** Would clicking this row open it right now? The row's own click rule
 *  (`OtherHomeRow`'s `canOpen`; a connected-server row always opens), read
 *  from the stores without a render — for the Cmd+1–9 shortcut on Home. */
export function homeRowOpenableNow(row: HomeRow): boolean {
  const parsed = parseHomeAddress(row.address)
  const hostKey = parsed?.host ?? ''
  const host = useConnectHostStore.getState()
  const { status, onConnected } = computeRowStatus(row, {
    activeHost: host.activeHost,
    hosts: host.hosts,
    connectionStatus: host.connectionStatus,
    projects: useProjectsStore.getState().projects,
    entry: hostPool.store.getState().entries[hostKey],
    sameServerAs: null,
  })
  if (onConnected && host.connectionStatus === 'connected' && status.kind === 'idle') return true
  if (isWebClient() && !onConnected) return false
  return status.kind !== 'not-found' && status.kind !== 'no-access'
}

/** The "same server as …" note for a row on the connected server (its
 *  status paints as the Agents row, so the note rides beside it). */
export function useConnectedRowNote(row: HomeRow): string | null {
  const parsed = parseHomeAddress(row.address)
  const label = useSameServerAs(parsed?.host ?? '')
  return label ? `Same server as ${label}` : null
}

// ── Polling (every row's server, through the pool) ───────────────────────

function useHomeStatusPoll(home: Home): void {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const hosts = useConnectHostStore((s) => s.hosts)
  const keys = useMemo(() => homeHostKeys(home).sort(), [home])
  const keysJoined = keys.join('\n')
  // A login landing (or dropping) for one of them re-checks right away.
  const loginsKey = hosts.map((h) => `${h.id}:${h.token.length > 0 ? 1 : 0}`).join('|')

  useEffect(() => {
    if (keys.length === 0) return
    let cancelled = false
    const timers = new Map<string, ReturnType<typeof setTimeout>>()
    const schedule = (key: string, ms: number): void => {
      const prev = timers.get(key)
      if (prev !== undefined) clearTimeout(prev)
      timers.set(key, setTimeout(() => void run(key), ms))
    }
    const run = async (key: string): Promise<void> => {
      if (cancelled) return
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') {
        schedule(key, HOME_POLL_MS)
        return
      }
      // This computer's daemon does not exist for the web client.
      if (key === LOCAL_HOME_HOST && isWebClient()) return
      // The connected server: readiness and instanceId only — its login,
      // presence and recovery are the window's own (ConnectionGate).
      const entry = await hostPool.check(key, { bootOnly: key === connectedKey })
      if (!cancelled) schedule(key, nextCheckDelayMs(entry))
    }
    for (const key of keys) void run(key)
    const onVisible = (): void => {
      if (document.visibilityState === 'visible') for (const key of keys) void run(key)
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      cancelled = true
      for (const t of timers.values()) clearTimeout(t)
      document.removeEventListener('visibilitychange', onVisible)
    }
    // keysJoined stands in for keys (stable string identity).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [keysJoined, connectedKey, loginsKey])
}

/** Rows on the connected server follow a workspace rename (handle moved:
 *  the stored id finds it) and pick up its current name. */
function useConnectedRowRepair(): void {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  useEffect(() => {
    if (connectionStatus !== 'connected' || projects.length === 0) return
    useHomesStore.getState().repairRows((row) => {
      const p = parseHomeAddress(row.address)
      if (!p || p.host !== connectedKey) return null
      const ws = findWorkspaceForRow(projects, row)
      if (!ws) return null
      const handle = workspaceHandle(ws)
      if (!handle) return null
      return { address: homeAddress(handle, connectedKey), workspaceId: ws.id, label: ws.name }
    })
  }, [projects, connectedKey, connectionStatus])
}

/** Mounted by App only while Home is on screen (not under Settings), so
 *  the status loop runs only while someone can see it. Renders nothing. */
export function HomeShellEffects(): null {
  const home = useHomesStore(selectedHome)
  useHomeStatusPoll(home)
  useConnectedRowRepair()
  useEffect(() => () => useHomeAddPickerStore.getState().setOpen(false), [])
  // vs-live P31: the Zen toggle row sits inside the picker's outside-click
  // area, so turning Zen on would leave the picker open under Zen (and back
  // on screen when Zen exits). Zen on closes it.
  const zenShown = useZenShown()
  useEffect(() => {
    if (zenShown) useHomeAddPickerStore.getState().setOpen(false)
  }, [zenShown])
  return null
}
