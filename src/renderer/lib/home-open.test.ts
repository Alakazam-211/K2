// Home M4 (MS55) — opening a Home row with "Open agents from other servers
// here" off and on. Off: a row on another server switches this window's
// server (today's H15 path). On: it opens that server's room in Home
// without switching. A row on the connected server always selects it in
// the window's room. 0.43.2 Z5/Z42 (T1.3 unit half): with it on, a server
// whose known version is below the floor (or has no version) switches the
// window with one toast per server per session; a server the pool has not
// read yet opens a room (the floor is applied when the read lands).

import { describe, it, expect, vi, beforeEach } from 'vitest'

const h = vi.hoisted(() => ({
  open: vi.fn(async () => ({})),
  showPrimary: vi.fn(),
  requestHostSelect: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/stores/home-rooms', () => ({
  homeRooms: { open: h.open, showPrimary: h.showPrimary },
}))
vi.mock('@/lib/home-pending-select', () => ({ requestHostSelect: h.requestHostSelect }))

import { openHomeRow } from '@/lib/home-open'
import { HOME_ROOM_FLOOR, HOME_ROOM_USABLE_FROM, homeRoomVerdict } from '@/lib/home-room-floor'
import { __resetOldServerToastsForTests } from '@/lib/home-switch'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry, PoolBoot } from '@/lib/host-pool'
import { useToastStore } from '@/stores/toast'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { usePageViewStore } from '@/stores/page-view'
import type { HomeRow } from '@/stores/homes'

const B: ConnectHost = {
  id: 'id-b',
  label: 'B',
  hostname: 'b.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: false,
  lastConnectedAt: null,
}

const remoteRow: HomeRow = { address: 'anna::b.k2.dev', workspaceId: 'pb', label: 'Anna' }
const localRow: HomeRow = { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' }

let pickHost: ReturnType<typeof vi.fn>
let setActiveProject: ReturnType<typeof vi.fn>

function boot(version: string | null): PoolBoot {
  return { phase: 'ready', ready: true, version, protocol: 1, instanceId: 'i', features: [], at: 1 }
}

/** The pool has read B's `/boot-status`. */
function poolKnowsB(version: string | null): void {
  const entry: HostEntry = {
    hostKey: 'b.k2.dev',
    saved: true,
    hostId: 'id-b',
    reach: 'live',
    boot: boot(version),
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1,
  }
  hostPool.store.setState({ entries: { 'b.k2.dev': entry } })
}

function toasts(): string[] {
  return useToastStore.getState().toasts.map((t) => t.message)
}

beforeEach(() => {
  hostPool.store.setState({ entries: {} })
  __resetOldServerToastsForTests()
  useToastStore.setState({ toasts: [] })
  h.open.mockClear()
  h.showPrimary.mockClear()
  h.requestHostSelect.mockClear()
  pickHost = vi.fn()
  setActiveProject = vi.fn()
  useConnectHostStore.setState({ hosts: [B], activeHost: 'local', pickHost } as never)
  useProjectsStore.setState({
    projects: [{ id: 'pl', name: 'cortana', path: '/ws/cortana', handle: 'cortana', workspaces: [] }],
    setActiveProject,
  } as never)
  usePageViewStore.getState().setPage('agents')
})

describe('openHomeRow — the setting off', () => {
  beforeEach(() => {
    useRemoteRoomsPreviewStore.setState({ enabled: false })
  })

  it('a row on another server switches this window to it and opens no room', () => {
    expect(openHomeRow(remoteRow)).toBe('switching')
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost.mock.calls[0][0]).toBe(B)
    expect(h.requestHostSelect).toHaveBeenCalledTimes(1)
    expect(h.requestHostSelect.mock.calls[0][0]).toBe('id-b')
    expect(h.open).not.toHaveBeenCalled()
  })

  it('a row on the connected server selects it in the window’s own room', () => {
    expect(openHomeRow(localRow)).toBe('selected')
    expect(setActiveProject).toHaveBeenCalledWith('pl')
    expect(h.showPrimary).toHaveBeenCalledTimes(1)
    expect(pickHost).not.toHaveBeenCalled()
    expect(h.open).not.toHaveBeenCalled()
  })
})

describe('openHomeRow — the setting on (M4, the 0.43.2 default)', () => {
  beforeEach(() => {
    useRemoteRoomsPreviewStore.setState({ enabled: true })
  })

  it('a row on another server opens its room on Home and never switches the window', () => {
    expect(openHomeRow(remoteRow)).toBe('room')
    expect(h.open).toHaveBeenCalledTimes(1)
    expect(h.open.mock.calls[0]).toEqual([remoteRow, 'b.k2.dev'])
    expect(pickHost).not.toHaveBeenCalled()
    expect(h.requestHostSelect).not.toHaveBeenCalled()
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('a row on a server that is not saved is refused (no room, no switch)', () => {
    const unknown: HomeRow = { address: 'x::nowhere.k2.dev', workspaceId: null, label: 'X' }
    expect(openHomeRow(unknown)).toBe('unknown-server')
    expect(h.open).not.toHaveBeenCalled()
    expect(pickHost).not.toHaveBeenCalled()
  })

  it('a row on the connected server still selects it in the window’s own room', () => {
    expect(openHomeRow(localRow)).toBe('selected')
    expect(setActiveProject).toHaveBeenCalledWith('pl')
    expect(h.showPrimary).toHaveBeenCalledTimes(1)
    expect(h.open).not.toHaveBeenCalled()
  })
})

describe('the floor (Z5, Z42; Q5)', () => {
  beforeEach(() => {
    useRemoteRoomsPreviewStore.setState({ enabled: true })
  })

  it('is 0.41.0; rooms are usable from 0.43.0', () => {
    expect(HOME_ROOM_FLOOR).toBe('0.41.0')
    expect(HOME_ROOM_USABLE_FROM).toBe('0.43.0')
    expect(homeRoomVerdict(null)).toBe('unknown')
    expect(homeRoomVerdict(boot(null))).toBe('switch')
    expect(homeRoomVerdict(boot('0.40.150'))).toBe('switch')
    expect(homeRoomVerdict(boot('0.41.0'))).toBe('view-only')
    expect(homeRoomVerdict(boot('0.41.6'))).toBe('view-only')
    expect(homeRoomVerdict(boot('0.43.0'))).toBe('use')
    expect(homeRoomVerdict(boot('0.43.2-rc1'))).toBe('use')
  })

  it('a known version below the floor switches the window, with one toast per server per session', () => {
    poolKnowsB('0.40.150')
    expect(openHomeRow(remoteRow)).toBe('switching')
    expect(h.open).not.toHaveBeenCalled()
    expect(h.showPrimary).toHaveBeenCalledTimes(1)
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost.mock.calls[0][0]).toBe(B)
    expect(h.requestHostSelect).toHaveBeenCalledTimes(1)
    expect(h.requestHostSelect.mock.calls[0][0]).toBe('id-b')
    expect(h.requestHostSelect.mock.calls[0][1]).toBe(remoteRow)
    expect(toasts()).toEqual(['B runs K2 0.40.150. It opens by switching this window. Update it to open it here.'])
    // Again in the same session: switches, no second toast.
    expect(openHomeRow(remoteRow)).toBe('switching')
    expect(pickHost).toHaveBeenCalledTimes(2)
    expect(toasts().length).toBe(1)
  })

  it('a server that reports no version switches too', () => {
    poolKnowsB(null)
    expect(openHomeRow(remoteRow)).toBe('switching')
    expect(h.open).not.toHaveBeenCalled()
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(toasts()).toEqual(['B runs an older K2. It opens by switching this window. Update it to open it here.'])
  })

  it('at the floor (a view-only room) and on a usable server, the row opens a room', () => {
    for (const v of ['0.41.0', '0.41.6', '0.43.0']) {
      poolKnowsB(v)
      expect([v, openHomeRow(remoteRow)]).toEqual([v, 'room'])
    }
    expect(h.open).toHaveBeenCalledTimes(3)
    expect(pickHost).not.toHaveBeenCalled()
    expect(toasts()).toEqual([])
  })

  it('a server the pool has not read yet opens a room (the floor is applied when the read lands, Z42)', () => {
    expect(hostPool.entry('b.k2.dev')).toBe(undefined)
    expect(openHomeRow(remoteRow)).toBe('room')
    expect(h.open).toHaveBeenCalledTimes(1)
    expect(pickHost).not.toHaveBeenCalled()
  })

  it('with the setting off, the floor plays no part: no toast', () => {
    useRemoteRoomsPreviewStore.setState({ enabled: false })
    poolKnowsB('0.40.150')
    expect(openHomeRow(remoteRow)).toBe('switching')
    expect(toasts()).toEqual([])
  })
})
