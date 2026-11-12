// @vitest-environment jsdom
// A finish or needs-you toast's View, clicked while Home is on screen, shows
// the agent: one that is not a Home row opens on the Agents page (it used to
// be selected under Home, where nothing showed it); a Home row opens on Home.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'

const h = vi.hoisted(() => ({
  open: vi.fn(),
  showPrimary: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
vi.mock('@/stores/home-rooms', async () => {
  const { create } = await import('zustand')
  const useHomeRoomsStore = create<{ shown: string | null }>(() => ({ shown: null }))
  h.open.mockImplementation(async (row: { address: string }) => {
    useHomeRoomsStore.setState({ shown: row.address })
    return {}
  })
  h.showPrimary.mockImplementation(() => useHomeRoomsStore.setState({ shown: null }))
  return {
    homeRooms: { open: h.open, showPrimary: h.showPrimary, store: useHomeRoomsStore },
    useHomeRoomsStore,
  }
})

import { navigateToRow } from './active-agents'
import { __resetPageSelectionForTests, installPageSelectionMemory } from '@/lib/page-selection'
import { openHomeRow } from '@/lib/home-open'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useGrokBotsStore } from '@/stores/grokbots'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useSettingsStore } from '@/stores/settings'

const rowA: HomeRow = { address: 'alpha::local', workspaceId: 'pa', label: 'Alpha' }
const rowB: HomeRow = { address: 'bee::local', workspaceId: 'pb', label: 'Bee' }

const project = (id: string, handle: string) => ({
  id,
  name: handle,
  path: `/ws/${handle}`,
  handle,
  workspaces: [{ id: `${id}-main`, name: 'main', type: 'main', navVisible: 0, worktreePath: null }],
})

/** An activity row for `projectId`'s agent (the toast's subject). */
const rowFor = (projectId: string) => ({ projectId, agentName: projectId, sessionId: `s-${projectId}` }) as never

let uninstall: (() => void) | null = null

beforeEach(() => {
  __resetPageSelectionForTests()
  h.open.mockClear()
  h.showPrimary.mockClear()
  useHomeRoomsStore.setState({ shown: null })
  useRemoteRoomsPreviewStore.setState({ enabled: true })
  useSettingsStore.setState({ workspaceSwitchFocus: 'composer', settingsOpen: false })
  useGrokBotsStore.setState({ open: null } as never)
  useConnectHostStore.setState({ hosts: [], activeHost: 'local', connectionStatus: 'connected' } as never)
  useProjectsStore.setState({
    projects: [project('pa', 'alpha'), project('pb', 'bee'), project('pc', 'cee')],
    activeProjectId: null,
    activeWorkspaceId: null,
    setActiveProject: (id: string | null) =>
      useProjectsStore.setState({ activeProjectId: id, activeWorkspaceId: id ? `${id}-main` : null }),
    setActiveWorkspace: (projectId: string, workspaceId: string) =>
      useProjectsStore.setState({ activeProjectId: projectId, activeWorkspaceId: workspaceId }),
  } as never)
  // Home holds A and B; C is only on Agents.
  useHomesStore.setState({ homes: [{ id: 'h1', name: 'Home', rows: [rowA, rowB] }], selectedId: 'h1' } as never)
  usePageViewStore.getState().setPage('agents')
  uninstall = installPageSelectionMemory()
})

afterEach(() => {
  uninstall?.()
  uninstall = null
})

describe("a toast's View, clicked on Home", () => {
  it('an agent that is not on Home opens on the Agents page', async () => {
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowA)

    navigateToRow(rowFor('pc'))

    await vi.waitFor(() => expect(usePageViewStore.getState().page).toBe('agents'))
    expect(useProjectsStore.getState().activeProjectId).toBe('pc')
  })

  it('a Home agent opens on Home', async () => {
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowA)

    navigateToRow(rowFor('pb'))

    await vi.waitFor(() => expect(useProjectsStore.getState().activeProjectId).toBe('pb'))
    expect(usePageViewStore.getState().page).toBe('home')
  })
})
