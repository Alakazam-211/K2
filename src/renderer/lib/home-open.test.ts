// Home M4 (MS55) — opening a Home row with "Remote rooms (preview)" off and
// on. Off: a row on another server switches this window's server (today's
// H15 path). On: it opens that server's room in Home without switching.
// A row on the connected server always selects it in the window's room.

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

beforeEach(() => {
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

describe('openHomeRow — Remote rooms (preview) off (today)', () => {
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

describe('openHomeRow — Remote rooms (preview) on (M4)', () => {
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
