// @vitest-environment jsdom
// Home and Agents each keep their own selected agent (Rosson 2026-10-16:
// selecting an agent on Home must not change the agent the Agents page
// had, even when the Home agent is on this computer, and back).
//
// Real stores (projects, homes, connect host, Grok Bots, page view); the
// projects store's select actions are stubbed to only set the selection.
// Two rooms stand in for the main area: the one for the active workspace
// is shown, the other hidden, as `AgentsShell` does.

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

import {
  __resetPageSelectionForTests,
  installPageSelectionMemory,
  openOnAgentsPage,
  roomPageOwner,
} from './page-selection'
import {
  __resetWorkspaceSwitchFocusForTests,
  installRoomPageSwitchFocus,
} from '@/lib/workspace-switch-focus'
import { openHomeRow } from '@/lib/home-open'
import { useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore } from '@/stores/page-view'
import { useHomesStore, type HomeRow } from '@/stores/homes'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { useGrokBotsStore } from '@/stores/grokbots'
import { useHomeRoomsStore } from '@/stores/home-rooms'

const remoteHost: ConnectHost = { id: 'b', label: 'B box', hostname: 'b.k2.dev', port: 443, secure: true, token: 'tok' } as ConnectHost

const rowA: HomeRow = { address: 'alpha::local', workspaceId: 'pa', label: 'Alpha' }
const rowB: HomeRow = { address: 'bee::local', workspaceId: 'pb', label: 'Bee' }
const rowRemote: HomeRow = { address: 'zed::b.k2.dev', workspaceId: null, label: 'Zed' }

const project = (id: string, handle: string) => ({
  id,
  name: handle,
  path: `/ws/${handle}`,
  handle,
  workspaces: [{ id: `${id}-main`, name: 'main', type: 'main', navVisible: 0, worktreePath: null }],
})

let uninstalls: Array<() => void> = []

/** One room per workspace, with its PTY bar and Thread composer; only the
 *  active workspace's room is displayed. */
function mountRooms(): Record<string, { thread: HTMLTextAreaElement; pty: HTMLTextAreaElement }> {
  document.body.innerHTML = ''
  const out: Record<string, { thread: HTMLTextAreaElement; pty: HTMLTextAreaElement; el: HTMLElement }> = {}
  for (const id of ['pa', 'pb', 'pc']) {
    const el = document.createElement('div')
    el.setAttribute('data-terminal-pane-wrapper', '')
    const ptyBar = document.createElement('div')
    ptyBar.setAttribute('data-compose-bar', '')
    const pty = document.createElement('textarea')
    ptyBar.appendChild(pty)
    el.appendChild(ptyBar)
    const slot = document.createElement('div')
    slot.setAttribute('data-testid', 'agent-session-thread-compose-slot')
    const bar = document.createElement('div')
    bar.setAttribute('data-compose-bar', '')
    const thread = document.createElement('textarea')
    bar.appendChild(thread)
    slot.appendChild(bar)
    el.appendChild(slot)
    document.body.appendChild(el)
    out[id] = { thread, pty, el }
  }
  const paint = (): void => {
    const active = useProjectsStore.getState().activeProjectId
    for (const [id, r] of Object.entries(out)) r.el.style.display = id === active ? '' : 'none'
  }
  paint()
  uninstalls.push(useProjectsStore.subscribe(paint))
  return out
}

async function frames(): Promise<void> {
  for (let i = 0; i < 2; i++) await new Promise<void>((r) => requestAnimationFrame(() => r()))
}

function active(): string | null {
  return useProjectsStore.getState().activeProjectId
}

function select(id: string): void {
  useProjectsStore.getState().setActiveProject(id)
}

beforeEach(() => {
  __resetPageSelectionForTests()
  __resetWorkspaceSwitchFocusForTests()
  document.body.innerHTML = ''
  h.open.mockClear()
  h.showPrimary.mockClear()
  useHomeRoomsStore.setState({ shown: null })
  useRemoteRoomsPreviewStore.setState({ enabled: true })
  useSettingsStore.setState({ workspaceSwitchFocus: 'composer', settingsOpen: false })
  useGrokBotsStore.setState({ open: null } as never)
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
  uninstalls = [installPageSelectionMemory()]
})

afterEach(() => {
  for (const u of uninstalls.splice(0)) u()
  __resetWorkspaceSwitchFocusForTests()
  document.body.innerHTML = ''
})

describe('Home and Agents keep their own selected agent', () => {
  it('a local agent picked on Home does not change Agents, and Home keeps it', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    expect(openHomeRow(rowB)).toBe('selected')
    expect(active()).toBe('pb')

    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pa')

    usePageViewStore.getState().setPage('home')
    expect(active()).toBe('pb')
  })

  it('an Agents-only agent (not a Home row) comes back on Agents after Home', () => {
    select('pc')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pc')
  })

  it('a pick on Agents does not change Home', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    usePageViewStore.getState().setPage('agents')
    select('pc')
    usePageViewStore.getState().setPage('home')
    expect(active()).toBe('pb')
    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pc')
  })

  it('a remote agent picked on Home does not change Agents, and Home still shows it', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    // Home first opens a local row (e.g. its top row), then the remote one.
    openHomeRow(rowB)
    expect(openHomeRow(rowRemote)).toBe('room')
    expect(useHomeRoomsStore.getState().shown).toBe(rowRemote.address)

    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pa')
    // Agents never closes Home's remote room.
    expect(useHomeRoomsStore.getState().shown).toBe(rowRemote.address)

    usePageViewStore.getState().setPage('home')
    expect(useHomeRoomsStore.getState().shown).toBe(rowRemote.address)
    expect(active()).toBe('pb')
  })

  it('pages in between (Projects, Wiki) keep each page its own', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    usePageViewStore.getState().setPage('projects')
    expect(active()).toBe('pb')
    expect(roomPageOwner()).toBe('home')
    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pa')
    usePageViewStore.getState().openWiki('/ws/alpha')
    usePageViewStore.getState().closeWiki()
    expect(active()).toBe('pa')
    usePageViewStore.getState().setPage('home')
    expect(active()).toBe('pb')
  })

  it('a Grok Bot chat opened on Home stays on Home', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    useGrokBotsStore.getState().openChat({ hostKey: 'local', agentId: 'g1' })
    usePageViewStore.getState().setPage('agents')
    expect(useGrokBotsStore.getState().open).toBe(null)
    expect(active()).toBe('pa')
    usePageViewStore.getState().setPage('home')
    expect(useGrokBotsStore.getState().open).toEqual({ hostKey: 'local', agentId: 'g1' })
  })

  it('a Grok Bot chat opened on Agents does not close Home\'s remote room', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowRemote)
    usePageViewStore.getState().setPage('agents')
    useGrokBotsStore.getState().openChat({ hostKey: 'local', agentId: 'g1' })
    expect(useHomeRoomsStore.getState().shown).toBe(rowRemote.address)
    usePageViewStore.getState().setPage('home')
    expect(useGrokBotsStore.getState().open).toBe(null)
    usePageViewStore.getState().setPage('agents')
    expect(useGrokBotsStore.getState().open).toEqual({ hostKey: 'local', agentId: 'g1' })
  })

  it('"Open in Agents" from Home selects on Agents and leaves Home its own', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    openOnAgentsPage('pc')
    expect(usePageViewStore.getState().page).toBe('agents')
    expect(active()).toBe('pc')
    usePageViewStore.getState().setPage('home')
    expect(active()).toBe('pb')
    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pc')
  })

  it('"Open in Agents" for an agent that is also a Home row, from another page', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowA)
    usePageViewStore.getState().setPage('projects')
    openOnAgentsPage('pb')
    expect(active()).toBe('pb')
    usePageViewStore.getState().setPage('home')
    expect(active()).toBe('pa')
  })

  it('a remembered agent that is gone is skipped', () => {
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    useProjectsStore.setState({ projects: [project('pb', 'bee'), project('pc', 'cee')] } as never)
    usePageViewStore.getState().setPage('agents')
    expect(active()).toBe('pb')
  })
})

describe('page-switch focus lands in each page\'s own agent', () => {
  beforeEach(() => {
    uninstalls.push(installRoomPageSwitchFocus())
  })

  it('back on Agents: A\'s Thread composer; back on Home: B\'s', async () => {
    const rooms = mountRooms()
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    await frames()
    expect(document.activeElement).toBe(rooms.pb.thread)

    usePageViewStore.getState().setPage('agents')
    await frames()
    expect(document.activeElement).toBe(rooms.pa.thread)

    usePageViewStore.getState().setPage('home')
    await frames()
    expect(document.activeElement).toBe(rooms.pb.thread)
  })

  it('with the Thread closed: the agent message box of each page\'s agent', async () => {
    const rooms = mountRooms()
    for (const r of Object.values(rooms)) r.thread.closest('[data-testid]')!.remove()
    select('pa')
    usePageViewStore.getState().setPage('home')
    openHomeRow(rowB)
    await frames()
    usePageViewStore.getState().setPage('agents')
    await frames()
    expect(document.activeElement).toBe(rooms.pa.pty)
  })
})
