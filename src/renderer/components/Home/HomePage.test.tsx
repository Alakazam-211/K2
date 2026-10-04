// @vitest-environment jsdom
//
// Home on the Agents shell (prd-home-v1 H2, Rosson 2026-09-30: "the Home
// UI should match the agents page UI"). The page is rendered through the
// real `AgentsShell` + `Layout` + `TopBar` + drawers; only the leaf
// surfaces are stubbed (the room's TerminalArea, the drawer bodies, the
// server switcher / page tabs, and the Agents `Sidebar` default export so
// the test can tell which sidebar is mounted).
//
// Asserted:
//   - Home uses the Agents shell: the same row shell (`AgentRowButton`,
//     `data-agent-row`), the same top bar with its drawer toggles, the
//     same drawers, and the same room area as Agents.
//   - A row on the connected server selects that workspace and the page
//     stays Home; the room then shows in the main area.
//   - With "Open agents from other servers here" off, a row on another
//     server switches through `pickHost` and the pending select lands on
//     Home (not Agents). Homes do not change. With the default on a mac
//     (nothing stored) it opens that server's room instead; on Windows the
//     default still switches (0.43.2 Z28, Q2).
//   - Nothing selected / empty Home shows the Agents empty state with Home
//     wording; Add Agent is one button that opens one picker with This
//     server + From a server.
//   - Status: connected row shows Agents' working glyph; another server
//     polls /boot-status + presence and shows live + its people; a server
//     with no login shows "sign in" and is never asked.

import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({
  default: () => null,
  hostDisplayAddress: (h: { hostname: string; port: number; secure: boolean }) =>
    h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`,
}))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({
  default: ({ children }: { children?: React.ReactNode }) => <>{children}</>,
}))
vi.mock('@/components/TopBar/TopBarUtilities', () => ({
  default: ({ leading, children }: { leading?: React.ReactNode; children?: React.ReactNode }) => (
    <>
      {leading}
      {children}
    </>
  ),
}))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/lib/titlebar-drag', () => ({
  titleBarDragOnMouseDown: () => undefined,
  titleBarOnDoubleClick: () => undefined,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({ found: false, dataUrl: null })),
  daemonCliPost: vi.fn(async () => ({})),
  // The pool's status checks take the per-server /cli slot (Home M2).
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
// The room + drawer bodies: the shell decides they mount; their insides
// have their own tests.
vi.mock('@/components/Terminal/TerminalArea', () => ({
  TerminalArea: ({ cwd }: { cwd: string }) => <div data-testid="terminal-area" data-cwd={cwd} />,
}))
vi.mock('@/components/FileTree/FileTree', () => ({
  default: ({ rootPath }: { rootPath: string }) => <div data-testid="drawer-files" data-root={rootPath} />,
}))
vi.mock('@/components/ChangesPanel/ChangesPanel', () => ({ default: () => <div data-testid="drawer-changes" /> }))
vi.mock('@/components/ChatHistory/ChatHistory', () => ({
  default: ({ projectPath }: { projectPath: string }) => <div data-testid="drawer-history" data-root={projectPath} />,
}))
vi.mock('@/components/WorkspacePanel/WorkspacePanel', () => ({ default: () => <div data-testid="drawer-workspace" /> }))
const menu = vi.hoisted(() => ({ next: null as string | null, items: [] as { id: string; label: string }[][] }))
vi.mock('@/lib/context-menu', () => ({
  showContextMenu: vi.fn(async (items: { id: string; label: string }[]) => {
    menu.items.push(items)
    return menu.next
  }),
}))
vi.mock('@/components/Sidebar/Sidebar', async () => {
  const real = await vi.importActual<typeof import('@/components/Sidebar/Sidebar')>('@/components/Sidebar/Sidebar')
  return { ...real, default: () => <div data-testid="agents-sidebar" /> }
})

const h = vi.hoisted(() => {
  function hookOf<T extends object>(state: T) {
    const hook = ((sel: (s: T) => unknown) => sel(state)) as ((sel: (s: T) => unknown) => unknown) & {
      getState: () => T
      setState: (p: Partial<T>) => void
    }
    hook.getState = () => state
    hook.setState = (p) => Object.assign(state, p)
    return hook
  }
  const activity = hookOf({ getProjectStatus: (id: string) => (id === 'pl' ? 'working' : 'idle') })
  const presence = hookOf({ roster: [] as unknown[], supported: true })
  return { hookOf, activity, presence }
})

vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  interface Ws {
    id: string
    name: string
    type: string
    navVisible: number
    worktreePath: string | null
  }
  interface Proj {
    id: string
    name: string
    path: string
    handle: string
    color: string
    iconUrl: string | null
    workspaces: Ws[]
  }
  const ws = (id: string, name: string): Ws => ({ id, name, type: 'main', navVisible: 0, worktreePath: null })
  const projects: Proj[] = [
    { id: 'pl', name: 'Cortana', path: '/w/cortana', handle: 'cortana', color: '#e06c75', iconUrl: null, workspaces: [ws('wl', 'main')] },
    { id: 'pn', name: 'Nova', path: '/w/nova', handle: 'nova', color: '#61afef', iconUrl: null, workspaces: [ws('wn', 'main')] },
  ]
  const useProjectsStore = create<{
    projects: Proj[]
    activeProjectId: string | null
    activeWorkspaceId: string | null
    setActiveProject: (id: string) => void
    setActiveWorkspace: (projectId: string, workspaceId: string) => void
  }>((set, get) => ({
    projects,
    activeProjectId: null,
    activeWorkspaceId: null,
    setActiveProject: (id) => {
      const p = get().projects.find((x) => x.id === id)
      if (!p) throw new Error(`setActiveProject: no project ${id}`)
      set({ activeProjectId: id, activeWorkspaceId: p.workspaces[0].id })
    },
    setActiveWorkspace: (projectId, workspaceId) => set({ activeProjectId: projectId, activeWorkspaceId: workspaceId }),
  }))
  return { useProjectsStore, __initialProjects: projects }
})
vi.mock('@/stores/active-agents', () => ({ useActiveAgentsStore: h.activity }))
vi.mock('@/stores/presence', async () => {
  const real = await vi.importActual<typeof import('@/stores/presence')>('@/stores/presence')
  return { ...real, usePresenceStore: h.presence }
})
vi.mock('@/stores/session-events', () => ({
  onAppHello: vi.fn(),
  onPresenceChanged: vi.fn(),
}))

import AgentsShell from '@/components/Layout/AgentsShell'
import { HomeShellEffects } from './home-room'
import { useHomesStore } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { usePanelsStore } from '@/stores/panels'
import { useSidebarStore } from '@/stores/sidebar'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { __resetHostPoolForTests } from '@/lib/host-pool-instance'
import { peekHostSelect, clearHostSelect, takeHostSelect } from '@/lib/home-pending-select'
import { LS_REMOTE_ROOMS_PREVIEW, readRemoteRoomsPreview, useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { homeRooms } from '@/stores/home-rooms'
import { otherRowOpenTitle } from './HomeSidebar'

const boxB: ConnectHost = {
  id: 'b',
  label: 'Box B',
  hostname: 'b.k2.dev',
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'tok-b',
  remember: true,
  lastConnectedAt: null,
}
const boxC: ConnectHost = { ...boxB, id: 'c', label: 'Box C', hostname: 'c.k2.dev', token: '' }

const fetchMock = vi.fn(async (url: string) => {
  if (url === 'https://b.k2.dev/boot-status') {
    return new Response(JSON.stringify({ version: '0.41.6', protocol: 1, phase: 'ready' }), { status: 200 })
  }
  if (url === 'https://b.k2.dev/cli/auth/whoami?token=tok-b') {
    return new Response(
      JSON.stringify({ username: 'rosson', owner: false, role: 'member', mustChangePassword: false }),
      { status: 200 },
    )
  }
  if (url === 'https://b.k2.dev/cli/presence/summary?token=tok-b') {
    return new Response(
      JSON.stringify({
        online: 2,
        workspaces: [
          {
            workspaceId: 'pb',
            handle: 'bee',
            name: 'Bee',
            path: '/srv/bee',
            count: 2,
            people: [
              { user: 'rosson', name: 'rosson', role: 'member' },
              { user: 'anna', name: 'anna', role: 'member' },
            ],
          },
        ],
      }),
      { status: 200 },
    )
  }
  throw new Error(`unexpected fetch ${url}`)
})

/** The App's Home wiring: the shell, plus the status loop App mounts
 *  only while Home is on screen. */
function Shell(): React.JSX.Element {
  const page = usePageViewStore((s) => s.page)
  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const cwd = activeWorkspace?.worktreePath ?? activeProject?.path ?? '~'
  return (
    <>
      <AgentsShell activeProject={activeProject} activeWorkspace={activeWorkspace} cwd={cwd} />
      {page === 'home' && <HomeShellEffects />}
    </>
  )
}

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  __resetConnectHostStoreForTests()
  useConnectHostStore.setState({ hosts: [boxB, boxC], connectionStatus: 'connected' })
  clearHostSelect()
  __resetHostPoolForTests()
  useProjectsStore.setState({ activeProjectId: null, activeWorkspaceId: null })
  useSidebarStore.setState({ isCollapsed: false })
  usePanelsStore.setState({
    leftPanelOpen: true,
    leftPanelTabs: ['files', 'workspace'],
    leftPanelActiveTab: 'files',
    rightPanelOpen: true,
    rightPanelTabs: ['history', 'changes'],
    rightPanelActiveTab: 'history',
  })
  const home = useHomesStore.getState().homes[0]
  for (const r of home.rows) useHomesStore.getState().removeRow(home.id, r.address)
  useHomesStore.getState().selectHome(home.id)
  useHomesStore.getState().addRow(home.id, { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' })
  useHomesStore.getState().addRow(home.id, { address: 'bee::b.k2.dev', workspaceId: 'pb', label: 'Bee' })
  useHomesStore.getState().addRow(home.id, { address: 'cee::c.k2.dev', workspaceId: 'pc', label: 'Cee' })
  usePageViewStore.getState().setPage('home')
})

afterEach(() => {
  cleanup()
  fetchMock.mockClear()
})

// A probe still in flight when a test ends keeps using the stub (a later
// request would otherwise reach the real network).
afterAll(() => {
  vi.unstubAllGlobals()
})

function homeSidebar(): HTMLElement {
  const el = document.querySelector('[data-home-sidebar]')
  if (!(el instanceof HTMLElement)) throw new Error('Home sidebar is not mounted')
  return el
}

function rowOf(label: string): HTMLElement {
  const el = within(homeSidebar()).getByText(label).closest('[data-agent-row]')
  if (!(el instanceof HTMLElement)) throw new Error(`no Agents row shell for ${label}`)
  return el
}

function roomArea(): HTMLElement {
  const el = document.querySelector('[data-room-area]')
  if (!(el instanceof HTMLElement)) throw new Error('room area is not mounted')
  return el
}

describe('Home — the Agents page shell', () => {
  it('Agents and Home mount the same shell; only the sidebar differs', () => {
    usePageViewStore.getState().setPage('agents')
    useProjectsStore.getState().setActiveProject('pl')
    const { rerender } = render(<Shell />)
    expect(screen.getByTestId('agents-sidebar')).toBeTruthy()
    expect(document.querySelector('[data-home-sidebar]')).toBeNull()
    expect(within(roomArea()).getByTestId('terminal-area')).toBeTruthy()

    act(() => usePageViewStore.getState().setPage('home'))
    rerender(<Shell />)
    expect(screen.queryByTestId('agents-sidebar')).toBeNull()
    expect(homeSidebar()).toBeTruthy()
    // The beta notice sits above the Home picker, on Home only.
    const notice = within(homeSidebar()).getByTestId('home-beta-notice')
    expect(notice.textContent).toBe('Home is inBeta')
    // Every Home row is the Agents row shell.
    const rows = homeSidebar().querySelectorAll('[data-agent-row]')
    expect(rows.length).toBe(3)
    // Same room, still mounted, and shown: Cortana is on this Home.
    expect(roomArea().style.display).toBe('')
    expect(within(roomArea()).getByTestId('terminal-area').getAttribute('data-cwd')).toBe('/w/cortana')
  })

  it('a selected local agent has the Agents drawers and drawer toggles on Home', () => {
    useProjectsStore.getState().setActiveProject('pl')
    render(<Shell />)
    // Top bar drawer toggles (the Agents TopBar).
    const left = screen.getByTitle('Toggle left panel')
    const right = screen.getByTitle('Toggle right panel')
    // Drawer bodies for the selected agent.
    expect(screen.getByTestId('drawer-files').getAttribute('data-root')).toBe('/w/cortana')
    expect(screen.getByTestId('drawer-history').getAttribute('data-root')).toBe('/w/cortana')
    expect(screen.getByText('Cortana', { selector: 'span.text-\\[var\\(--color-text-secondary\\)\\]' })).toBeTruthy()

    fireEvent.click(left)
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(screen.queryByTestId('drawer-files')).toBeNull()
    fireEvent.click(right)
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
    expect(screen.queryByTestId('drawer-history')).toBeNull()
  })

  it('clicking a row on the connected server selects it and stays on Home', () => {
    render(<Shell />)
    // Nothing selected yet: Agents empty state, Home wording, no drawers.
    expect(screen.getByText('Pick an agent on this Home')).toBeTruthy()
    expect(screen.queryByTestId('drawer-files')).toBeNull()

    fireEvent.click(rowOf('Cortana'))
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    expect(useProjectsStore.getState().activeWorkspaceId).toBe('wl')
    expect(usePageViewStore.getState().page).toBe('home')
    // The room opens in the main area; the sidebar still shows the Home roster.
    expect(within(roomArea()).getByTestId('terminal-area').getAttribute('data-cwd')).toBe('/w/cortana')
    expect(screen.queryByText('Pick an agent on this Home')).toBeNull()
    expect(rowOf('Cortana').getAttribute('aria-current')).toBe('true')
    expect(homeSidebar().querySelectorAll('[data-agent-row]').length).toBe(3)
  })

  it('the active workspace not on this Home shows the empty state, room hidden', () => {
    useProjectsStore.getState().setActiveProject('pn')
    render(<Shell />)
    expect(screen.getByText('Pick an agent on this Home')).toBeTruthy()
    expect(roomArea().style.display).toBe('none')
    expect(screen.getByText('No workspace selected')).toBeTruthy()
  })

  it('an empty Home shows the empty state with Add Agent wording', () => {
    const home = useHomesStore.getState().homes[0]
    for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
    render(<Shell />)
    expect(screen.getByText('Add an agent to get started', { selector: '[data-room-empty] p' })).toBeTruthy()
    expect(within(homeSidebar()).getByText('No agents on this Home yet')).toBeTruthy()
  })

  it('with the setting off, clicking a row on another server switches through pickHost and lands on Home', async () => {
    useRemoteRoomsPreviewStore.setState({ enabled: false })
    const pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost })
    render(<Shell />)
    await act(async () => {
      fireEvent.click(rowOf('Bee'))
    })
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost.mock.calls[0][0]).toMatchObject({ id: 'b', hostname: 'b.k2.dev' })
    expect(peekHostSelect()).toMatchObject({ hostId: 'b', row: { address: 'bee::b.k2.dev', workspaceId: 'pb' } })
    // Homes do not change.
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toEqual([
      'cortana::local',
      'bee::b.k2.dev',
      'cee::c.k2.dev',
    ])

    // B's list lands: the restore takes the requested workspace, and the
    // follow-up lands on Home (never Agents).
    usePageViewStore.getState().setPage('agents')
    const pick = takeHostSelect(boxB, [{ id: 'pb', name: 'Bee', handle: 'bee' }])
    if (!pick) throw new Error('pending select did not match box B')
    expect(pick.workspace.id).toBe('pb')
    if (!pick.onSelected) throw new Error('pending select has no follow-up')
    pick.onSelected()
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('the default on a mac (nothing stored): clicking a row on another server opens its room, no switch (Z1, Z28)', async () => {
    localStorage.removeItem(LS_REMOTE_ROOMS_PREVIEW)
    const platform = vi.spyOn(navigator, 'platform', 'get').mockReturnValue('MacIntel')
    try {
      useRemoteRoomsPreviewStore.setState({ enabled: readRemoteRoomsPreview() })
      expect(useRemoteRoomsPreviewStore.getState().enabled).toBe(true)
      const pickHost = vi.fn()
      useConnectHostStore.setState({ pickHost })
      render(<Shell />)
      await act(async () => {
        fireEvent.click(rowOf('Bee'))
      })
      expect(pickHost).not.toHaveBeenCalled()
      expect(peekHostSelect()).toBe(null)
      expect(useConnectHostStore.getState().activeHost).toBe('local')
      expect(homeRooms.store.getState().shown).toBe('bee::b.k2.dev')
      expect(homeRooms.store.getState().entries['bee::b.k2.dev']?.hostKey).toBe('b.k2.dev')
      expect(usePageViewStore.getState().page).toBe('home')
    } finally {
      platform.mockRestore()
      await act(async () => {
        await homeRooms.closeAll()
      })
    }
  })

  it('the default on Windows (nothing stored) still switches the window (Q2, until G-Win)', async () => {
    localStorage.removeItem(LS_REMOTE_ROOMS_PREVIEW)
    const platform = vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32')
    try {
      useRemoteRoomsPreviewStore.setState({ enabled: readRemoteRoomsPreview() })
      expect(useRemoteRoomsPreviewStore.getState().enabled).toBe(false)
      const pickHost = vi.fn()
      useConnectHostStore.setState({ pickHost })
      render(<Shell />)
      await act(async () => {
        fireEvent.click(rowOf('Bee'))
      })
      expect(pickHost).toHaveBeenCalledTimes(1)
      expect(homeRooms.store.getState().entries['bee::b.k2.dev']).toBe(undefined)
    } finally {
      platform.mockRestore()
    }
  })

  it('row states: working glyph (connected), live + presence (other), sign in (no login)', async () => {
    render(<Shell />)
    // Connected row = the Agents row: working braille spinner.
    expect(rowOf('Cortana').querySelector('.braille-spinner')).not.toBeNull()
    expect(rowOf('Cortana').querySelector('[data-machine-chip]')).toBeNull()
    // Other servers carry a machine chip.
    expect(rowOf('Bee').querySelector('[data-machine-chip]')?.textContent).toBe('Box B')
    expect(rowOf('Cee').querySelector('[data-machine-chip]')?.textContent).toBe('Box C')
    expect(rowOf('Cee').querySelector('[data-row-status]')?.getAttribute('data-row-status')).toBe('sign-in')

    await waitFor(() =>
      expect(rowOf('Bee').querySelector('[data-row-status]')?.getAttribute('data-row-status')).toBe('live'),
    )
    // Presence: anna is there; you (rosson on B) are not shown.
    expect(within(rowOf('Bee')).getByTitle('anna')).toBeTruthy()
    expect(within(rowOf('Bee')).queryByTitle(/rosson/)).toBeNull()

    const urls = fetchMock.mock.calls.map((c) => c[0])
    expect(urls).toContain('https://b.k2.dev/boot-status')
    expect(urls).toContain('https://b.k2.dev/cli/presence/summary?token=tok-b')
    // No login held for C: only its PUBLIC readiness is read (Home M2
    // pool) — never an authed route, and never a login POST.
    const cUrls = urls.filter((u) => u.includes('c.k2.dev'))
    expect(cUrls.length).toBeGreaterThan(0)
    expect(cUrls.every((u) => u === 'https://c.k2.dev/boot-status')).toBe(true)
  })

  it('Add Agent is the one bottom button; its picker has This server and From a server', () => {
    render(<Shell />)
    const bar = screen.getByText('Add Agent').closest('div')
    if (!bar) throw new Error('no bottom bar')
    // The bar: Add Agent + the collapse button, like Add Workspace.
    expect(Array.from(bar.querySelectorAll(':scope > button')).map((b) => b.textContent?.trim() || b.getAttribute('aria-label'))).toEqual([
      'Add Agent',
      'Collapse workspaces sidebar',
    ])

    fireEvent.click(screen.getByText('Add Agent'))
    const picker = screen.getByRole('menu', { name: 'Add Agent' })
    expect(within(picker).getByText('This server')).toBeTruthy()
    expect(within(picker).getByText('From a server')).toBeTruthy()

    fireEvent.click(within(picker).getByText('This server'))
    expect(picker.textContent).toContain('Agents on This computer')
    expect(picker.textContent).toContain('Nova')
    expect(picker.textContent).not.toContain('Cortana')
    fireEvent.click(within(picker).getByText('Nova'))
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toContain('nova::local')
  })

  it('rows 1–9 carry the pinned area badge (Cmd+N order); the picker shows ⌘ 1-9 and each Home its ⌥⌘N', () => {
    const home = useHomesStore.getState().homes[0]
    // 3 rows from beforeEach (connected Cortana, Bee, Cee) + Nova + 7 more on
    // the no-login server = 11 rows of three kinds.
    useHomesStore.getState().addRow(home.id, { address: 'nova::local', workspaceId: 'pn', label: 'Nova' })
    for (let i = 1; i <= 7; i++) {
      useHomesStore.getState().addRow(home.id, { address: `x${i}::c.k2.dev`, workspaceId: null, label: `X${i}` })
    }
    const extra = [useHomesStore.getState().createHome('Second'), useHomesStore.getState().createHome('Third')]
    useHomesStore.getState().selectHome(home.id)
    try {
      render(<Shell />)
      const rowEls = Array.from(homeSidebar().querySelectorAll('[data-home-row]'))
      expect(rowEls.length).toBe(11)
      const badges = rowEls.map((el) => {
        const found = el.querySelectorAll('[data-shortcut-badge]')
        expect(found.length).toBeLessThanOrEqual(1)
        return found[0]?.textContent ?? null
      })
      expect(badges).toEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', null, null])
      // Badge N is on rows[N - 1], the row Cmd+N opens.
      expect(rowEls.map((el) => el.getAttribute('data-home-row'))).toEqual(
        useHomesStore.getState().homes[0].rows.map((r) => r.address),
      )
      // Connected rows (SingleProjectItem) and other-server rows both carry it.
      expect(rowOf('Cortana').querySelector('[data-shortcut-badge="1"]')).not.toBeNull()
      expect(rowOf('Bee').querySelector('[data-shortcut-badge="2"]')).not.toBeNull()
      expect(rowOf('Nova').querySelector('[data-shortcut-badge="4"]')).not.toBeNull()
      // The modifier: the Agents pinned header's `⌘ 1-9` beside the count.
      const pickerButton = within(homeSidebar()).getByTitle('Pick a Home')
      const hint = pickerButton.querySelector('[data-shortcut-hint]')
      if (!hint) throw new Error('no row chord hint on the Home picker')
      expect(hint.getAttribute('data-shortcut-hint')).toBe('⌘ 1-9')
      expect(hint.textContent).toBe('⌘ 1-9')
      expect(hint.querySelector('.key-symbol')?.textContent).toBe('⌘')

      fireEvent.click(pickerButton)
      const items = within(homeSidebar()).getAllByRole('menuitemradio')
      expect(items.map((el) => el.querySelector('[data-shortcut-badge]')?.textContent ?? null)).toEqual([
        '⌥⌘1',
        '⌥⌘2',
        '⌥⌘3',
      ])
    } finally {
      for (const id of extra) if (id) useHomesStore.getState().deleteHome(id)
    }
  })

  it('right-click Remove from Home drops the row only', async () => {
    menu.next = 'home-remove'
    menu.items = []
    render(<Shell />)
    await act(async () => {
      fireEvent.contextMenu(rowOf('Cee'))
    })
    expect(menu.items[0].map((i) => i.label)).toContain('Remove from Home')
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toEqual(['cortana::local', 'bee::b.k2.dev'])
    // The connected row's menu is the workspace menu + remove.
    menu.next = null
    await act(async () => {
      fireEvent.contextMenu(rowOf('Cortana'))
    })
    expect(menu.items[1].map((i) => i.id)).toEqual(['home-settings', 'home-wiki', 'home-sep', 'home-remove'])
  })

  it('From a server lists saved servers, minus the connected one', () => {
    render(<Shell />)
    fireEvent.click(screen.getByText('Add Agent'))
    const picker = screen.getByRole('menu', { name: 'Add Agent' })
    fireEvent.click(within(picker).getByText('From a server'))
    expect(within(picker).getByText('Box B')).toBeTruthy()
    expect(within(picker).getByText('Box C')).toBeTruthy()
    expect(within(picker).queryByText('This computer')).toBeNull()
  })
})

describe('the tooltip of a row on another server (Z3, Z5)', () => {
  it('says what the click does: here, here view only, or switch', () => {
    expect(otherRowOpenTitle(true, 'use', 'Bee', 'Box B')).toBe('Open Bee from Box B here')
    expect(otherRowOpenTitle(true, 'unknown', 'Bee', 'Box B')).toBe('Open Bee from Box B here')
    expect(otherRowOpenTitle(true, 'view-only', 'Bee', 'Box B')).toBe('Open Bee from Box B here, view only')
    expect(otherRowOpenTitle(true, 'switch', 'Bee', 'Box B')).toBe('Switch this window to Box B and open Bee')
    expect(otherRowOpenTitle(false, 'use', 'Bee', 'Box B')).toBe('Switch this window to Box B and open Bee')
  })
})
