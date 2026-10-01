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
// Row status polls other servers every 30s, only while Home is on screen
// and the window is visible (`HomeShellEffects` is mounted only then).

import { useEffect, useMemo } from 'react'
import { create } from 'zustand'
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
  probeHost,
  resolveRowStatus,
  useHomeProbeStore,
  type RowStatus,
} from '@/lib/home-status'
import { credsForHomeHost } from '@/lib/home-creds'

/** True when Home is the page and the window's active workspace is a row
 *  of the selected Home on the connected server — the room is shown. */
export function useHomeRoomSelected(): boolean {
  const onHome = usePageViewStore((s) => s.page === 'home')
  const home = useHomesStore(selectedHome)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const projects = useProjectsStore((s) => s.projects)
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  return useMemo(() => {
    if (!onHome || !activeProjectId) return false
    return home.rows.some((row) => {
      const p = parseHomeAddress(row.address)
      if (!p || p.host !== connectedKey) return false
      return findWorkspaceForRow(projects, row)?.id === activeProjectId
    })
  }, [onHome, home.rows, activeProjectId, projects, connectedKey])
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

/** Status of a row that does not paint as a live Agents row: a row on
 *  another server (live / offline / sign in), or a connected-server row
 *  whose workspace is gone or still loading. */
export function useRowStatus(row: HomeRow): { status: RowStatus; place: string | null; onConnected: boolean } {
  const parsed = parseHomeAddress(row.address)
  const activeHost = useConnectHostStore((s) => s.activeHost)
  const hosts = useConnectHostStore((s) => s.hosts)
  const connectionStatus = useConnectHostStore((s) => s.connectionStatus)
  const projects = useProjectsStore((s) => s.projects)
  const hostKey = parsed?.host ?? ''
  const probe = useHomeProbeStore((s) => s.probes[hostKey])
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
      probe,
      self: isLocal ? 'owner' : saved?.username || 'owner',
    }),
    place,
    onConnected,
  }
}

// ── Polling (other servers) ──────────────────────────────────────────────

function useHomeStatusPoll(home: Home): void {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const hosts = useConnectHostStore((s) => s.hosts)
  const otherKeys = useMemo(() => {
    const keys = new Set<string>()
    for (const r of home.rows) {
      const p = parseHomeAddress(r.address)
      if (p && p.host !== connectedKey) keys.add(p.host)
    }
    return [...keys].sort()
  }, [home.rows, connectedKey])
  const otherKeysJoined = otherKeys.join('\n')
  // A login landing (or dropping) for one of them re-polls right away.
  const loginsKey = hosts.map((h) => `${h.id}:${h.token.length > 0 ? 1 : 0}`).join('|')

  useEffect(() => {
    if (otherKeys.length === 0) return
    let cancelled = false
    const run = async (): Promise<void> => {
      if (typeof document !== 'undefined' && document.visibilityState === 'hidden') return
      const savedHosts = useConnectHostStore.getState().hosts
      await Promise.all(
        otherKeys.map(async (key) => {
          const c = await credsForHomeHost(key, savedHosts)
          if (cancelled) return
          const setProbe = useHomeProbeStore.getState().setProbe
          if (c.kind === 'unsaved') return
          if (c.kind === 'unreachable') {
            setProbe(key, { reach: 'offline', auth: 'ok', presence: null, at: Date.now() })
            return
          }
          // No login: the row says "Sign in" without asking the server.
          if (!c.creds.token) return
          const probe = await probeHost(c.creds)
          if (!cancelled) setProbe(key, probe)
        }),
      )
    }
    void run()
    const timer = setInterval(() => void run(), HOME_POLL_MS)
    const onVisible = (): void => {
      if (document.visibilityState === 'visible') void run()
    }
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      cancelled = true
      clearInterval(timer)
      document.removeEventListener('visibilitychange', onVisible)
    }
    // otherKeysJoined stands in for otherKeys (stable string identity).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [otherKeysJoined, loginsKey])
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
  return null
}
