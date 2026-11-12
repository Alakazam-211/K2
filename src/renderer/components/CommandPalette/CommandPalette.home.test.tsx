// @vitest-environment jsdom
// Cmd+K on Home: picking an agent shows that agent. Home shows only its own
// rows, so an agent that is not a Home row opens on the Agents page (it used
// to be selected under Home, where nothing showed it). A Home row opens on
// Home, as clicking the row does, and a remote room on screen gives way.
//
// Real stores (projects, homes, connect host, Grok Bots, page view, focus
// groups); the projects store's select actions only set the selection.

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

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

import CommandPalette from './CommandPalette'
import { __resetPageSelectionForTests, installPageSelectionMemory } from '@/lib/page-selection'
import { openHomeRow } from '@/lib/home-open'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore, type HomeRow } from '@/stores/homes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useGrokBotsStore } from '@/stores/grokbots'
import { useHomeRoomsStore } from '@/stores/home-rooms'
import { useFocusGroupsStore } from '@/stores/focus-groups'
import { useSettingsStore } from '@/stores/settings'

const remoteHost: ConnectHost = { id: 'b', label: 'B box', hostname: 'b.k2.dev', port: 443, secure: true, token: 'tok' } as ConnectHost

const rowA: HomeRow = { address: 'alpha::local', workspaceId: 'pa', label: 'Alpha' }
const rowB: HomeRow = { address: 'bee::local', workspaceId: 'pb', label: 'Bee' }
const rowRemote: HomeRow = { address: 'zed::b.k2.dev', workspaceId: null, label: 'Zed' }

const project = (id: string, handle: string) => ({
  id,
  name: handle,
  path: `/ws/${handle}`,
  handle,
  color: '#888888',
  focusGroupId: null,
  iconUrl: null,
  workspaces: [{ id: `${id}-main`, name: 'main', type: 'main', navVisible: 0, worktreePath: null }],
})

let uninstall: (() => void) | null = null

function active(): { projectId: string | null; workspaceId: string | null } {
  const s = useProjectsStore.getState()
  return { projectId: s.activeProjectId, workspaceId: s.activeWorkspaceId }
}

function page(): string {
  return usePageViewStore.getState().page
}

/** Open the palette, type `query`, press Enter on the first result. */
function pick(query: string): void {
  act(() => useCommandPaletteStore.getState().open())
  const input = screen.getByPlaceholderText(/Search workspaces/)
  fireEvent.change(input, { target: { value: query } })
  fireEvent.keyDown(input, { key: 'Enter' })
}

beforeEach(() => {
  __resetPageSelectionForTests()
  h.open.mockClear()
  h.showPrimary.mockClear()
  useHomeRoomsStore.setState({ shown: null })
  useRemoteRoomsPreviewStore.setState({ enabled: true })
  useSettingsStore.setState({ workspaceSwitchFocus: 'composer', settingsOpen: false })
  useGrokBotsStore.setState({ open: null } as never)
  useFocusGroupsStore.setState({ focusGroups: [], focusGroupsEnabled: false } as never)
  useCommandPaletteStore.setState({ isOpen: false })
  useConnectHostStore.setState({ hosts: [remoteHost], activeHost: 'local', connectionStatus: 'connected' } as never)
  useProjectsStore.setState({
    projects: [project('pa', 'alpha'), project('pb', 'bee'), project('pc', 'cee')],
    activeProjectId: null,
    activeWorkspaceId: null,
    setActiveProject: (id: string | null) =>
      useProjectsStore.setState({ activeProjectId: id, activeWorkspaceId: id ? `${id}-main` : null }),
    setActiveWorkspace: (projectId: string, workspaceId: string) =>
      useProjectsStore.setState({ activeProjectId: projectId, activeWorkspaceId: workspaceId }),
  } as never)
  // Home holds A and B (both on this computer) and a remote agent; C is
  // only on Agents.
  useHomesStore.setState({ homes: [{ id: 'h1', name: 'Home', rows: [rowA, rowB, rowRemote] }], selectedId: 'h1' } as never)
  usePageViewStore.getState().setPage('agents')
  uninstall = installPageSelectionMemory()
  render(<CommandPalette />)
})

afterEach(() => {
  cleanup()
  uninstall?.()
  uninstall = null
})

describe('Cmd+K on Home', () => {
  it('an agent that is not on Home opens on the Agents page, selected', () => {
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    expect(active().projectId).toBe('pb')

    pick('cee')

    expect(page()).toBe('agents')
    expect(active()).toEqual({ projectId: 'pc', workspaceId: 'pc-main' })
    expect(useCommandPaletteStore.getState().isOpen).toBe(false)
    // Home keeps its own selection for when the person goes back.
    usePageViewStore.getState().setPage('home')
    expect(active().projectId).toBe('pb')
  })

  it('an agent that is not on Home, picked while a remote Home room is shown, still opens on Agents', () => {
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowRemote)
    expect(useHomeRoomsStore.getState().shown).toBe(rowRemote.address)

    pick('cee')

    expect(page()).toBe('agents')
    expect(active().projectId).toBe('pc')
    // Agents never closes Home's remote room.
    expect(useHomeRoomsStore.getState().shown).toBe(rowRemote.address)
  })

  it('a Home agent opens on Home, as clicking its row does', () => {
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowA)

    pick('bee')

    expect(page()).toBe('home')
    expect(active()).toEqual({ projectId: 'pb', workspaceId: 'pb-main' })
  })

  it('a Home agent picked while a remote room or a Grok Bot chat covers Home gives the room back', () => {
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowRemote)
    pick('alpha')
    expect(page()).toBe('home')
    expect(active().projectId).toBe('pa')
    expect(useHomeRoomsStore.getState().shown).toBe(null)

    useGrokBotsStore.getState().openChat({ hostKey: 'local', agentId: 'g1' })
    pick('bee')
    expect(page()).toBe('home')
    expect(active().projectId).toBe('pb')
    expect(useGrokBotsStore.getState().open).toBe(null)
  })
})

describe('Cmd+K elsewhere', () => {
  it('on Agents it selects the agent there', () => {
    useProjectsStore.getState().setActiveProject('pa')
    pick('cee')
    expect(page()).toBe('agents')
    expect(active().projectId).toBe('pc')
  })

  it('on a page without a room (Tickets) it goes to Agents with the agent', () => {
    useProjectsStore.getState().setActiveProject('pa')
    usePageViewStore.getState().setPage('feedback')
    pick('cee')
    expect(page()).toBe('agents')
    expect(active().projectId).toBe('pc')
  })
})
