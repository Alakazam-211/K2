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
//   - With "Open other servers' agents in Home" off, a row on another
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
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
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
  titleBarOnContextMenu: () => undefined,
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
vi.mock('@/components/AgentPanel/AgentPanel', () => ({ default: () => <div data-testid="drawer-agent" /> }))
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
  const activity = hookOf({ agents: new Map(), liveSessionCwds: new Set<string>() })
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
vi.mock('@/stores/active-agents', () => ({
  useActiveAgentsStore: h.activity,
  projectHasLiveSession: (cwds: Set<string>, path: string) => cwds.has(path),
}))
vi.mock('@/stores/presence', async () => {
  const real = await vi.importActual<typeof import('@/stores/presence')>('@/stores/presence')
  return { ...real, usePresenceStore: h.presence }
})
vi.mock('@/stores/session-events', () => ({
  // Subscriptions answer their unsubscribe, as the real ones do.
  onAppHello: vi.fn(() => () => undefined),
  onPresenceChanged: vi.fn(() => () => undefined),
  // 0.46.8: person chips follow their server's picture changes.
  onPersonPictureChanged: vi.fn(() => () => undefined),
  // The setup checklist in the Home sidebar and rail re-reads on these.
  onFirstRunChanged: vi.fn(() => () => undefined),
  onLlmAccountsChanged: vi.fn(() => () => undefined),
  onProjectsChanged: vi.fn(() => () => undefined),
  onTunnelStatusChanged: vi.fn(() => () => undefined),
  onTunnelIdentityChanged: vi.fn(() => () => undefined),
  onPairCodeChanged: vi.fn(() => () => undefined),
}))

import AgentsShell from '@/components/Layout/AgentsShell'
import { HomeShellEffects, useHomeAddPickerStore } from './home-room'
import { useHomesStore } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { usePanelsStore } from '@/stores/panels'
import { useSidebarStore } from '@/stores/sidebar'
import { useProjectsStore } from '@/stores/projects'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'
import { parseFirstRunDoc, resetFirstRunForTests, useFirstRunStore } from '@/stores/first-run'
import { daemonCliPost } from '@/lib/daemon-cli'
import { __seedActivityForTests } from '@/stores/activity'
import { __resetHostPoolForTests, hostPool } from '@/lib/host-pool-instance'
import { peekHostSelect, clearHostSelect, takeHostSelect } from '@/lib/home-pending-select'
import { LS_REMOTE_ROOMS_PREVIEW, readRemoteRoomsPreview, useRemoteRoomsPreviewStore } from '@/lib/remote-rooms-preview'
import { homeRooms } from '@/stores/home-rooms'
import { HOME_NAV_EDGE_TINT, HOME_SETUP_SLOT_CLASS, otherRowOpenTitle } from './HomeSidebar'
import { TAB_STRIP_HEIGHT } from '@/lib/tab-strip'
import { NAV_COLLAPSE_GAP_PX } from '@/components/Sidebar/SidebarCollapseButton'
import { DRAWER_HEADER_BUTTON_CLASS } from '@/components/Layout/DrawerTabBarToggle'
import { __resetAddPickerListingsForTests } from './HomeAddPanels'
import { ADD_TO_HOME_MARGIN, ADD_TO_HOME_MAX, ADD_TO_HOME_SHEET_BELOW_PX, ADD_TO_HOME_SIZE_CLASS } from './AddToHomeModal'
import { useCommandPaletteStore } from '@/stores/command-palette'
import { HOME_AGENT_COLORS_KEY, useHomeAvatarStore } from '@/lib/home-avatars'

// jsdom has neither CSS.escape nor scrollIntoView; the picker's list uses
// both for its keyboard row.
if (typeof globalThis.CSS === 'undefined') {
  ;(globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v.replace(/["\\]/g, '\\$&') }
}
Element.prototype.scrollIntoView = vi.fn()

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

// agent-unread-tickets-v1: what box B's /boot-status lists and its summary's
// agentActivity (both absent by default: an older server).
const boxBState: { features?: string[]; agentActivity?: unknown[] } = {}

const fetchMock = vi.fn(async (url: string) => {
  // boot-status-views-v1: a held login sends its token on /boot-status.
  if (url === 'https://b.k2.dev/boot-status?token=tok-b') {
    return new Response(
      JSON.stringify({
        version: '0.41.6',
        protocol: 1,
        phase: 'ready',
        ...(boxBState.features ? { features: boxBState.features } : {}),
      }),
      { status: 200 },
    )
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
        ...(boxBState.agentActivity ? { agentActivity: boxBState.agentActivity } : {}),
      }),
      { status: 200 },
    )
  }
  if (url === 'https://b.k2.dev/cli/projects/list?token=tok-b') {
    return new Response(
      JSON.stringify([
        { id: 'pb', name: 'Bee', handle: 'bee', path: '/srv/bee', color: '#123456', iconUrl: null },
        { id: 'pq', name: 'Quill', handle: 'quill', path: '/srv/quill', color: '#654321', iconUrl: null },
      ]),
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

beforeEach(async () => {
  vi.stubGlobal('fetch', fetchMock)
  // S5: the connected server's daemon says Cortana's workspace is working.
  __seedActivityForTests(primaryScope(), {
    workspaces: [
      {
        projectId: 'pl',
        workspacePath: '/w/cortana',
        display: 'working',
        counts: { working: 1, monitoring: 0, waiting: 0, unverifiable: 0, idle: 0 },
        since: null,
      },
    ],
  })
  __resetConnectHostStoreForTests()
  useConnectHostStore.setState({ hosts: [boxB, boxC], connectionStatus: 'connected' })
  // Boot as the app does (ConnectionGate): the pool reads no saved login
  // before the keychain pass.
  await useConnectHostStore.getState().hydrateFromDisk()
  clearHostSelect()
  __resetHostPoolForTests()
  __resetAddPickerListingsForTests()
  localStorage.removeItem(HOME_AGENT_COLORS_KEY)
  useHomeAvatarStore.setState({ colors: {} })
  useProjectsStore.setState({ activeProjectId: null, activeWorkspaceId: null })
  useSidebarStore.setState({ isCollapsed: false })
  usePanelsStore.setState({
    leftPanelOpen: true,
    leftPanelTabs: ['files', 'workspace'],
    leftPanelActiveTab: 'files',
    rightPanelOpen: true,
    rightPanelTabs: ['history', 'agent'],
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
  delete boxBState.features
  delete boxBState.agentActivity
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
    // Home is out of beta: no notice above the picker (Rosson 2026-10-16).
    expect(within(homeSidebar()).queryByTestId('home-beta-notice')).toBeNull()
    // Every Home row is the Agents row shell.
    const rows = homeSidebar().querySelectorAll('[data-agent-row]')
    expect(rows.length).toBe(3)
    // Line 2 (tag + status + number) sits 2px below the name line.
    for (const row of rows) {
      const line2 = row.querySelector('[data-agent-row-line2]')
      if (!line2) throw new Error('Home row has no line 2')
      expect(line2.classList.contains('mt-0.5')).toBe(true)
    }
    // Same room, still mounted, and shown: Cortana is on this Home.
    expect(roomArea().style.display).toBe('')
    expect(within(roomArea()).getByTestId('terminal-area').getAttribute('data-cwd')).toBe('/w/cortana')
  })

  it('a selected local agent has the Agents drawers on Home (toggled from the tab bars, not the top bar)', () => {
    useProjectsStore.getState().setActiveProject('pl')
    render(<Shell />)
    // 0.46.0: no top bar drawer toggles on Home either; the drawers open
    // and close from the room / drawer tab bars (stubbed here), i.e. the
    // window's panels store.
    expect(screen.queryByTitle('Toggle left panel')).toBeNull()
    expect(screen.queryByTitle('Toggle right panel')).toBeNull()
    // Drawer bodies for the selected agent.
    expect(screen.getByTestId('drawer-files').getAttribute('data-root')).toBe('/w/cortana')
    expect(screen.getByTestId('drawer-history').getAttribute('data-root')).toBe('/w/cortana')
    expect(screen.getByText('Cortana', { selector: 'span.text-\\[var\\(--color-text-secondary\\)\\]' })).toBeTruthy()

    act(() => usePanelsStore.getState().toggleLeftPanel())
    expect(usePanelsStore.getState().leftPanelOpen).toBe(false)
    expect(screen.queryByTestId('drawer-files')).toBeNull()
    act(() => usePanelsStore.getState().toggleRightPanel())
    expect(usePanelsStore.getState().rightPanelOpen).toBe(false)
    expect(screen.queryByTestId('drawer-history')).toBeNull()
  })

  it('clicking a row on the connected server selects it and stays on Home', () => {
    render(<Shell />)
    // Entering Home opened the top row; clear it to click one.
    act(() => useProjectsStore.setState({ activeProjectId: null, activeWorkspaceId: null }))
    // Nothing selected: Agents empty state, Home wording, no drawers.
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
    render(<Shell />)
    // After entry (which opened the top row), the Agents page's pick moves
    // to an agent not on this Home.
    act(() => useProjectsStore.getState().setActiveProject('pn'))
    expect(screen.getByText('Pick an agent on this Home')).toBeTruthy()
    expect(roomArea().style.display).toBe('none')
    expect(screen.getByText('No workspace selected')).toBeTruthy()
  })

  it('an empty group says so in the nav, with a + that opens Add to Home', () => {
    const home = useHomesStore.getState().homes[0]
    for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
    render(<Shell />)
    expect(screen.getByText('Add an agent to get started', { selector: '[data-room-empty] p' })).toBeTruthy()
    // The group's own empty state, centred in the list: its name, the hint
    // and a small + that opens Add to Home (focus comes back to it).
    const note = homeSidebar().querySelector('[data-home-empty]') as HTMLElement
    expect(note.getAttribute('role')).toBe('status')
    expect(note.className).toContain('absolute inset-0 flex flex-col items-center justify-center')
    expect(note.querySelector('b')?.textContent).toBe('Group 1 is empty')
    expect(within(note).getByText('Add agents with + below.')).toBeTruthy()
    expect(within(homeSidebar()).queryByText('No agents on this Home yet')).toBeNull()
    const plus = within(note).getByRole('button', { name: 'Add to Group 1' })
    expect(plus.getAttribute('title')).toBe('Add to Home')
    expect(plus.getAttribute('data-home-add-open')).toBe('empty')
    fireEvent.click(plus)
    expect(screen.getByRole('dialog', { name: 'Add to Home' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('dialog-close'))
    expect(document.activeElement).toBe(plus)
  })

  it('an empty group in the rail shows only a +, its title saying the group is empty', () => {
    const home = useHomesStore.getState().homes[0]
    for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
    useSidebarStore.setState({ isCollapsed: true })
    render(<Shell />)
    const stage = document.querySelector('[data-home-rail-stage]') as HTMLElement
    const buttons = within(stage).getAllByRole('button')
    expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual(['Group 1 is empty. Add to Home'])
    expect(buttons[0].getAttribute('title')).toBe('Group 1 is empty: add agents')
    fireEvent.click(buttons[0])
    expect(screen.getByRole('dialog', { name: 'Add to Home' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('dialog-close'))
    expect(document.activeElement).toBe(buttons[0])
  })

  it('the roster scrolls between the Home picker and the bottom bar, its ends fading in the nav\'s own colour', () => {
    render(<Shell />)
    const nav = homeSidebar()
    const frame = within(nav).getByTestId('home-nav-list')
    // Between the picker and the Add Agent bar, in the list's old place.
    const header = nav.querySelector('[data-home-header]') as HTMLElement
    const footer = nav.querySelector('[data-nav-footer]') as HTMLElement
    expect(header.compareDocumentPosition(frame) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(frame.compareDocumentPosition(footer) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(frame.className).toContain('flex-1 min-h-0')
    // The rows scroll inside it; one band at each end, never read aloud.
    const list = frame.querySelector('[data-scroll-edges-list]') as HTMLElement
    expect(list.querySelectorAll('[data-agent-row]').length).toBe(3)
    expect(list.className).toContain('overflow-y-auto')
    const bands = frame.querySelectorAll(':scope > [data-scroll-edge][aria-hidden="true"]')
    expect(Array.from(bands).map((b) => b.getAttribute('data-scroll-edge'))).toEqual(['top', 'bottom'])
    // Three rows fit: no band shows.
    expect([frame.dataset.edgeTop, frame.dataset.edgeBottom]).toEqual(['0', '0'])
    // Tinted with the nav tile's fill, never black.
    expect(frame.style.getPropertyValue('--k2-edge-tint')).toBe(HOME_NAV_EDGE_TINT)
    expect(HOME_NAV_EDGE_TINT).toBe('var(--tile-fill, var(--color-bg-surface))')
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

  it('an explicit off on Windows still switches the window (the 0.45.5 default is on)', async () => {
    localStorage.removeItem(LS_REMOTE_ROOMS_PREVIEW)
    const platform = vi.spyOn(navigator, 'platform', 'get').mockReturnValue('Win32')
    try {
      // Nothing stored: on, Windows included (0.45.5).
      expect(readRemoteRoomsPreview()).toBe(true)
      localStorage.setItem(LS_REMOTE_ROOMS_PREVIEW, '0')
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
    // Connected row = the Agents row: the working cell grid in its working
    // slot (plan-agent-status-unify-v1).
    expect(rowOf('Cortana').querySelector('[data-slot="working"] [data-agent-working]')).not.toBeNull()
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
    expect(urls).toContain('https://b.k2.dev/boot-status?token=tok-b')
    expect(urls).toContain('https://b.k2.dev/cli/presence/summary?token=tok-b')
    // No login held for C: only its PUBLIC readiness is read (Home M2
    // pool) — never an authed route, and never a login POST.
    const cUrls = urls.filter((u) => u.includes('c.k2.dev'))
    expect(cUrls.length).toBeGreaterThan(0)
    expect(cUrls.every((u) => u === 'https://c.k2.dev/boot-status')).toBe(true)
  })

  it('a row that says Sign in opens the sign-in for THAT server, as the room\'s Sign in button does', async () => {
    useRemoteRoomsPreviewStore.setState({ enabled: true })
    const signInForManagement = vi.fn()
    const pickHost = vi.fn()
    useConnectHostStore.setState({ signInForManagement, pickHost })
    try {
      render(<Shell />)
      expect(rowOf('Cee').querySelector('[data-row-action]')?.textContent).toBe('Sign in')
      await act(async () => {
        fireEvent.click(rowOf('Cee').querySelector('[data-row-action]') as Element)
      })
      expect(signInForManagement).toHaveBeenCalledTimes(1)
      expect(signInForManagement.mock.calls[0][0]).toMatchObject({ id: 'c', hostname: 'c.k2.dev' })
      // Non-switching: the window stays on its own server, the room is Cee's.
      expect(pickHost).not.toHaveBeenCalled()
      expect(homeRooms.store.getState().shown).toBe('cee::c.k2.dev')

      // A row with a login (Bee) opens its room and never asks to sign in.
      await act(async () => {
        fireEvent.click(rowOf('Bee'))
      })
      expect(homeRooms.store.getState().shown).toBe('bee::b.k2.dev')
      expect(signInForManagement).toHaveBeenCalledTimes(1)
    } finally {
      await act(async () => {
        await homeRooms.closeAll()
      })
    }
  })

  it('the collapsed rail: a Sign in row\'s icon opens the sign-in for that server too', async () => {
    useRemoteRoomsPreviewStore.setState({ enabled: true })
    useSidebarStore.setState({ isCollapsed: true })
    const signInForManagement = vi.fn()
    const pickHost = vi.fn()
    useConnectHostStore.setState({ signInForManagement, pickHost })
    try {
      render(<Shell />)
      const icon = document.querySelector('[data-home-rail-row="cee::c.k2.dev"]')
      if (!(icon instanceof HTMLElement)) throw new Error('no rail icon for Cee')
      expect(icon.getAttribute('data-row-status')).toBe('sign-in')
      await act(async () => {
        fireEvent.click(icon)
      })
      expect(signInForManagement).toHaveBeenCalledTimes(1)
      expect(signInForManagement.mock.calls[0][0]).toMatchObject({ id: 'c', hostname: 'c.k2.dev' })
      expect(pickHost).not.toHaveBeenCalled()
    } finally {
      await act(async () => {
        await homeRooms.closeAll()
      })
    }
  })

  // agent-unread-tickets-v1 (0.46.4): a remote row draws your unread /
  // waiting and the agent's tickets from its server's summary, only when
  // that server reports the feature.
  const bInbox = [
    {
      workspaceId: 'pb',
      status: 'idle',
      display: 'idle',
      heartbeat: false,
      live: false,
      busy: false,
      counts: { working: 0, monitoring: 0, waiting: 0, unverifiable: 0, idle: 0 },
      unread: 3,
      waiting: 1,
      muted: false,
      ticketsWaiting: 2,
      ticketsWaitingMe: 1,
    },
  ]

  it('remote row: unread, waiting and ticket badges from a server with agent-unread-tickets-v1', async () => {
    boxBState.features = ['activity-live-busy', 'agent-unread-tickets-v1']
    boxBState.agentActivity = bInbox
    render(<Shell />)
    await waitFor(() => expect(rowOf('Bee').querySelector('[data-remote-row-badges]')).not.toBeNull())
    const bee = rowOf('Bee')
    expect(bee.querySelector('[data-unread-count]')?.getAttribute('data-unread-count')).toBe('3')
    expect(bee.querySelector('[data-unread-waiting]')?.getAttribute('data-unread-waiting')).toBe('1')
    const tickets = bee.querySelector('[data-ticket-count]')
    expect(tickets?.getAttribute('data-ticket-count')).toBe('2')
    expect(tickets?.getAttribute('title')).toBe('2 tickets waiting (1 on you)')
    // The sign-in row (no login) never shows any.
    expect(rowOf('Cee').querySelector('[data-remote-row-badges]')).toBeNull()
  })

  it('remote row: an older server (no agent-unread-tickets-v1) shows no badges, even with the fields', async () => {
    boxBState.features = ['activity-live-busy']
    boxBState.agentActivity = bInbox
    render(<Shell />)
    await waitFor(() =>
      expect(rowOf('Bee').querySelector('[data-row-status]')?.getAttribute('data-row-status')).toBe('live'),
    )
    // The summary (with the fields) has landed in the pool: the row still
    // draws none.
    await waitFor(() => expect(hostPool.store.getState().entries['b.k2.dev']?.activity?.[0]?.unread).toBe(3))
    expect(rowOf('Bee').querySelector('[data-remote-row-badges]')).toBeNull()
    expect(rowOf('Bee').querySelector('[data-unread-count]')).toBeNull()
    expect(rowOf('Bee').querySelector('[data-ticket-count]')).toBeNull()
  })

  // thread-read-all-v1 (0.46.5): right-click → Mark all as read on a remote
  // row reads every Thread of that agent ON ITS SERVER, for this window's
  // login there, then the row follows that server's summary.
  it('remote row: Mark all as read posts thread/read-all to the row’s server, then the badge follows its summary', async () => {
    boxBState.features = ['activity-live-busy', 'agent-unread-tickets-v1', 'thread-read-all-v1']
    boxBState.agentActivity = bInbox
    render(<Shell />)
    await waitFor(() => expect(rowOf('Bee').querySelector('[data-unread-count]')?.getAttribute('data-unread-count')).toBe('3'))
    const post = vi.mocked(daemonCliPost)
    post.mockClear()
    const readAll = (): unknown[][] => post.mock.calls.filter((c) => c[1] === 'thread/read-all')
    const answer = {
      ok: true,
      changed: true,
      projectId: 'pb',
      marked: ['conv-now', 'conv-earlier'],
      unread: 0,
      waiting: 1,
      muted: false,
      total: 1,
      waitingTotal: 1,
    }
    post.mockImplementation((async (_scope: unknown, path: string) =>
      path === 'thread/read-all' ? answer : {}) as unknown as typeof daemonCliPost)
    // B's truth after the read: nothing unread, its card still waits.
    boxBState.agentActivity = [{ ...bInbox[0], unread: 0 }]
    menu.items = []
    menu.next = 'mark-all-read'
    await act(async () => {
      fireEvent.contextMenu(rowOf('Bee'))
    })
    expect(menu.items[0].map((i) => i.label)[0]).toBe('Mark all as read')
    await waitFor(() => expect(readAll()).toHaveLength(1))
    const [scope, path, body] = readAll()[0] as unknown as [{ hostKey: string }, string, unknown]
    expect([scope.hostKey, path, body]).toEqual(['b.k2.dev', 'thread/read-all', { projectId: 'pb' }])
    await waitFor(() => expect(rowOf('Bee').querySelector('[data-unread-count]')).toBeNull())
    expect(rowOf('Bee').querySelector('[data-unread-waiting]')?.getAttribute('data-unread-waiting')).toBe('1')
    menu.next = null
    post.mockImplementation((async () => ({})) as unknown as typeof daemonCliPost)
  })

  it('remote row: no Mark all as read from a server without thread-read-all-v1', async () => {
    boxBState.features = ['activity-live-busy', 'agent-unread-tickets-v1']
    boxBState.agentActivity = bInbox
    render(<Shell />)
    await waitFor(() => expect(rowOf('Bee').querySelector('[data-unread-count]')).not.toBeNull())
    menu.items = []
    menu.next = null
    await act(async () => {
      fireEvent.contextMenu(rowOf('Bee'))
    })
    expect(menu.items[0].map((i) => i.id)).not.toContain('mark-all-read')
  })

  it('remote row: no Mark all as read when only a card waits (reading never answers a card)', async () => {
    boxBState.features = ['activity-live-busy', 'agent-unread-tickets-v1', 'thread-read-all-v1']
    boxBState.agentActivity = [{ ...bInbox[0], unread: 0 }]
    render(<Shell />)
    await waitFor(() => expect(rowOf('Bee').querySelector('[data-unread-waiting]')).not.toBeNull())
    menu.items = []
    menu.next = null
    await act(async () => {
      fireEvent.contextMenu(rowOf('Bee'))
    })
    expect(menu.items[0].map((i) => i.id)).not.toContain('mark-all-read')
  })

  it('the bottom row is the ⌘K field and +; + opens Add to Home on the current group, one list per source', async () => {
    render(<Shell />)
    const home = useHomesStore.getState().homes[0]
    const row = homeSidebar().querySelector('[data-home-footer-row]') as HTMLElement
    expect(within(homeSidebar()).queryByText('Add Agent')).toBeNull()
    const field = within(row).getByRole('textbox', { name: 'Search workspaces (⌘K)' })
    const plus = within(row).getByRole('button', { name: 'Add to Home' })
    expect(field.compareDocumentPosition(plus) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(plus.getAttribute('aria-haspopup')).toBe('dialog')
    expect(row.className).toContain('gap-2')

    fireEvent.click(plus)
    const dialog = screen.getByRole('dialog', { name: 'Add to Home' })
    expect(dialog.hasAttribute('data-add-to-home-modal')).toBe(true)
    const addsTo = dialog.querySelector('[data-add-to]') as HTMLElement
    expect(addsTo.firstChild?.textContent).toBe('Adds to')
    expect(addsTo.querySelector('b')?.textContent).toBe(home.name)
    // The group's own glyph (here its number), not read aloud.
    expect(addsTo.querySelector('[aria-hidden="true"]')?.textContent).toBe('1')
    // One kind to add: no tab bar.
    expect(within(dialog).queryByRole('tablist')).toBeNull()
    // A section per source: this computer (connected), then each saved server.
    const groups = within(dialog).getAllByRole('group')
    expect(groups.map((g) => g.getAttribute('aria-label'))).toEqual(['This computer', 'Box B', 'Box C'])
    expect(groups[0].querySelector('[data-add-source-head]')?.textContent).toBe('This computerconnected')
    // An agent already in the group says so and can't be picked.
    const cortana = within(groups[0]).getByRole('option', { name: /Cortana/ })
    expect(cortana.getAttribute('aria-disabled')).toBe('true')
    expect(cortana.querySelector('[data-add-in-group]')?.textContent).toBe(`In ${home.name}`)
    fireEvent.click(cortana)
    expect(screen.getByRole('dialog', { name: 'Add to Home' })).toBe(dialog)
    // Picking adds to the current group, closes, and gives focus back to +.
    const nova = within(groups[0]).getByRole('option', { name: /Nova/ })
    expect(nova.getAttribute('aria-disabled')).toBeNull()
    expect(nova.querySelector('[data-add-go]')?.textContent).toBe('Add')
    fireEvent.click(nova)
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toContain('nova::local')
    expect(screen.queryByRole('dialog', { name: 'Add to Home' })).toBeNull()
    expect(document.activeElement).toBe(plus)
    // The new row gets a brief wash.
    expect(homeSidebar().querySelector('[data-home-row="nova::local"]')?.hasAttribute('data-just-added')).toBe(true)
  })

  it('Add to Home: the search matches name, path and handle; ↓ skips taken agents; Esc closes (T2.3, P26, P35)', async () => {
    render(<Shell />)
    const plus = within(homeSidebar()).getByRole('button', { name: 'Add to Home' })
    fireEvent.click(plus)
    const dialog = screen.getByRole('dialog', { name: 'Add to Home' })
    const input = within(dialog).getByRole('textbox', { name: 'Search agents' })
    expect((input as HTMLInputElement).placeholder).toBe('Search agents...')
    const shown = (): string[] =>
      within(within(dialog).getByRole('group', { name: 'This computer' })).queryAllByRole('option').map((o) => o.getAttribute('aria-label') ?? '')

    fireEvent.change(input, { target: { value: '/w/nov' } })
    expect(shown()).toEqual(['Nova, add to Group 1'])
    fireEvent.change(input, { target: { value: 'cortana' } })
    expect(shown()).toEqual(['Cortana, already in Group 1'])
    fireEvent.change(input, { target: { value: 'zzz' } })
    expect(within(dialog).queryByRole('group', { name: 'This computer' })).toBeNull()
    expect(dialog.querySelector('[data-add-empty]')?.textContent).toBe('No agents match “zzz”')

    // The search box takes focus when it opens.
    await act(async () => {
      await new Promise((r) => requestAnimationFrame(() => r(null)))
    })
    expect(document.activeElement).toBe(input)

    // ↓ skips the taken Cortana and lands on Nova; the list is one tab stop.
    fireEvent.change(input, { target: { value: '' } })
    fireEvent.keyDown(input, { key: 'ArrowDown' })
    const nova = within(dialog).getByRole('option', { name: /Nova/ })
    expect(document.activeElement).toBe(nova)
    expect(nova.tabIndex).toBe(0)
    expect(within(dialog).getByRole('option', { name: /Cortana/ }).tabIndex).toBe(-1)

    // Esc closes it, and focus goes back to +.
    fireEvent.keyDown(nova, { key: 'Escape' })
    expect(screen.queryByRole('dialog', { name: 'Add to Home' })).toBeNull()
    expect(document.activeElement).toBe(plus)
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).not.toContain('nova::local')
  })

  it('Zen shown (here: the chord, no click on Home) closes Add to Home (P31, G6)', async () => {
    const { toggleZenFromEscape, exitZen } = await import('@/lib/zen/zen-view')
    const { zenAvailable } = await import('@/lib/zen/zen-platform')
    if (!zenAvailable()) throw new Error('Zen is not available in this test environment')
    render(<Shell />)
    fireEvent.click(within(homeSidebar()).getByRole('button', { name: 'Add to Home' }))
    expect(screen.getByRole('dialog', { name: 'Add to Home' })).toBeTruthy()
    try {
      await act(async () => {
        toggleZenFromEscape()
      })
      expect(useHomeAddPickerStore.getState().open).toBe(false)
    } finally {
      await act(async () => {
        exitZen()
      })
    }
  })

  it('rows 1–9 carry the pinned area badge (Cmd+N order); no ⌘ 1-9 hint; each Home its ⌥⌘N', () => {
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
      // plan-agent-status-unify-v1: the number column runs 1–9, then 0 (the
      // Cmd+0 row), then a muted dash so the column never collapses.
      expect(badges).toEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', null])
      expect(rowEls.map((el) => el.querySelector('[data-slot="number"]')?.textContent ?? null)).toEqual([
        '1', '2', '3', '4', '5', '6', '7', '8', '9', '0', '-',
      ])
      // Badge N is on rows[N - 1], the row Cmd+N opens.
      expect(rowEls.map((el) => el.getAttribute('data-home-row'))).toEqual(
        useHomesStore.getState().homes[0].rows.map((r) => r.address),
      )
      // Connected rows (SingleProjectItem) and other-server rows both carry it.
      expect(rowOf('Cortana').querySelector('[data-shortcut-badge="1"]')).not.toBeNull()
      expect(rowOf('Bee').querySelector('[data-shortcut-badge="2"]')).not.toBeNull()
      expect(rowOf('Nova').querySelector('[data-shortcut-badge="4"]')).not.toBeNull()
      // No `⌘ 1-9` hint in the header: it holds the current group's name
      // (its one child, the full width) and the "…" for its card.
      expect(homeSidebar().querySelector('[data-home-row-hint]')).toBeNull()
      expect(homeSidebar().querySelector('[data-shortcut-hint]')).toBeNull()
      const header = homeSidebar().querySelector('[data-home-header]')
      if (!(header instanceof HTMLElement)) throw new Error('no Home header')
      expect(header.children.length).toBe(1)
      expect(header.querySelector('[data-group-name-header]')?.className).toContain('w-full')
      expect(within(header).getByRole('button', { name: `Edit groups (${home.name})` })).toBeTruthy()

      // The group icons on their own row right above Add Agent | « (no
      // divider anywhere): each group's ⌥⌘N in its label's shortcuts.
      const bar = within(homeSidebar()).getByRole('toolbar', { name: 'Groups' })
      expect(homeSidebar().querySelector('[data-nav-footer]')?.contains(bar)).toBe(false)
      const groupRow = homeSidebar().querySelector('[data-home-group-row]') as HTMLElement
      expect(groupRow.contains(bar)).toBe(true)
      expect(groupRow.className).not.toContain('border')
      // Setup finished: nothing between the group icons and the bottom bar.
      expect(groupRow.parentElement?.nextElementSibling).toBe(homeSidebar().querySelector('[data-nav-footer]'))
      expect(groupRow.parentElement?.className).not.toContain('border')
      expect(Array.from(bar.querySelectorAll('[data-group-id]')).map((b) => b.getAttribute('aria-keyshortcuts'))).toEqual([
        'Alt+Meta+1',
        'Alt+Meta+2',
        'Alt+Meta+3',
      ])
    } finally {
      for (const id of extra) if (id) useHomesStore.getState().deleteHome(id)
    }
  })

  it('Get started sits under the group icons, above the bottom bar, as the button and as the open list', () => {
    const setDoc = (over: Record<string, unknown>): void => {
      const doc = parseFirstRunDoc({
        enabled: true,
        hidden: false,
        finished: false,
        rows: [
          { id: 'llm_sign_in', state: 'todo', opens: 'llm_sign_in' },
          { id: 'tunnel', state: 'todo', opens: 'tunnel' },
        ],
        ...over,
      })
      act(() => useFirstRunStore.setState({ entries: { [primaryScope().id]: { doc, error: null } } }))
    }
    try {
      setDoc({ hidden: true })
      render(<Shell />)
      const nav = homeSidebar()
      const footer = nav.querySelector('[data-home-footer]') as HTMLElement
      const bottomBar = nav.querySelector('[data-nav-footer]') as HTMLElement
      // The button: group icons, then Get started, then the bottom bar.
      const button = nav.querySelector('[data-setup-checklist-collapsed]') as HTMLElement
      expect(within(button).getByTestId('setup-get-started')).toBeTruthy()
      expect(footer.nextElementSibling).toBe(button)
      expect(button.nextElementSibling).toBe(bottomBar)
      expect(footer.contains(nav.querySelector('[data-home-group-row]'))).toBe(true)
      // One gap, NAV_COLLAPSE_GAP_PX (8px, Tailwind's 2), from the icons to
      // Get started's top edge and from its bottom edge to the bar, in both
      // states: the icon row's bottom padding and the bar's top padding.
      // Neither depends on which part is drawn, so a fold (both parts drawn)
      // never changes them at its start or end. The closed button drops its
      // own padding.
      expect(NAV_COLLAPSE_GAP_PX).toBe(8)
      const cls = (el: Element): string[] => (el as HTMLElement).className.split(/\s+/)
      expect(cls(footer)).toEqual(expect.arrayContaining(['pt-1', 'pb-2', 'has-[+[data-nav-footer]]:pb-0']))
      expect(cls(bottomBar)).toEqual(expect.arrayContaining(['pt-2', 'pb-3']))
      expect(bottomBar.className).not.toContain('[[data-setup-checklist-collapsed]+&]')
      expect(footer.className).not.toContain('has-[+[data-setup-checklist]]')
      expect(cls(button)).toContain(HOME_SETUP_SLOT_CLASS)
      expect(HOME_SETUP_SLOT_CLASS).toBe('[&[data-setup-checklist-collapsed]>div]:py-0')
      expect(button.matches('[data-setup-checklist-collapsed]')).toBe(true)
      expect(button.firstElementChild?.tagName).toBe('DIV')
      // The open list sits in the same place, still a child of the nav's
      // column (its 45% cap is of the nav).
      setDoc({})
      const list = nav.querySelector('[data-setup-checklist]') as HTMLElement
      expect(list.parentElement).toBe(nav)
      expect(list.className).toContain('max-h-[45%]')
      expect(footer.nextElementSibling).toBe(list)
      expect(list.nextElementSibling).toBe(bottomBar)
      // The open list's top edge is its line (border-t), right after the
      // icon row's padding; the slot class leaves it alone.
      expect(list.className.split(/\s+/)).toContain('border-t')
      expect(list.matches('[data-setup-checklist-collapsed]')).toBe(false)
    } finally {
      resetFirstRunForTests()
    }
  })

  it('the top row is the tab strip\'s height: name, count, then "…" and « as drawer header buttons', () => {
    render(<Shell />)
    const home = useHomesStore.getState().homes[0]
    const row = homeSidebar().querySelector('[data-group-name-header]') as HTMLElement
    // The shared tab-strip token, never a number of its own, with the
    // tab strip's bottom line.
    expect(row.className.split(/\s+/)).toContain(TAB_STRIP_HEIGHT)
    expect(row.className).toContain('border-b border-[var(--color-border)]')
    expect(row.className).not.toMatch(/min-h-|\bpy-|\bpt-|\bpb-/)
    // Name (takes the room, truncates), count, then the two buttons, edge to edge.
    const name = row.querySelector('[data-group-name]') as HTMLElement
    expect(name.textContent).toBe(home.name)
    expect(name.className).toMatch(/\bflex-1\b/)
    expect(name.className).toMatch(/\btruncate\b/)
    expect(name.nextElementSibling?.getAttribute('data-group-count')).toBe('')
    const buttons = Array.from(row.querySelectorAll(':scope > button'))
    expect(buttons.map((b) => b.getAttribute('aria-label'))).toEqual([`Edit groups (${home.name})`, 'Collapse workspaces sidebar'])
    for (const b of buttons) {
      expect(b.hasAttribute('data-drawer-header-button')).toBe(true)
      expect(b.className).toContain(DRAWER_HEADER_BUTTON_CLASS)
    }
    expect(buttons[1].className).toContain('-ml-2')
    // "…" opens the Groups dialog on the current group; « collapses the nav.
    fireEvent.click(buttons[0])
    expect(document.querySelector('[data-group-detail]')?.getAttribute('data-group-detail')).toBe(home.id)
    fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' })
    fireEvent.click(buttons[1])
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
  })

  it('the top row has no « until setup is finished (as the footer\'s « was)', () => {
    const doc = parseFirstRunDoc({ enabled: true, hidden: true, finished: false, rows: [{ id: 'tunnel', state: 'todo', opens: 'tunnel' }] })
    act(() => useFirstRunStore.setState({ entries: { [primaryScope().id]: { doc, error: null } } }))
    try {
      render(<Shell />)
      const row = homeSidebar().querySelector('[data-group-name-header]') as HTMLElement
      expect(within(row).queryByRole('button', { name: 'Collapse workspaces sidebar' })).toBeNull()
      expect(within(row).getByRole('button', { name: /^Edit groups/ })).toBeTruthy()
    } finally {
      resetFirstRunForTests()
    }
  })

  it('the header has no palette button and no ⌘ 1-9 hint, with rows or without (T1.1, T1.2)', () => {
    render(<Shell />)
    expect(within(homeSidebar()).queryByTitle('Command Palette (⌘K)')).toBeNull()
    expect(homeSidebar().querySelector('[data-home-row-hint]')).toBeNull()
    expect(homeSidebar().querySelector('[data-shortcut-hint]')).toBeNull()
    cleanup()

    const home = useHomesStore.getState().homes[0]
    for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
    render(<Shell />)
    expect(homeSidebar().querySelector('[data-home-row-hint]')).toBeNull()
    expect(homeSidebar().querySelector('[data-shortcut-hint]')).toBeNull()
  })

  it('HomeSidebar no longer imports the palette store; App still binds ⌘K (T1.1)', () => {
    const dir = dirname(fileURLToPath(import.meta.url))
    const sidebar = readFileSync(join(dir, 'HomeSidebar.tsx'), 'utf8')
    expect(sidebar).not.toContain('stores/command-palette')
    expect(sidebar).not.toContain('useCommandPaletteStore')
    const app = readFileSync(join(dir, '..', '..', 'App.tsx'), 'utf8')
    const at = app.indexOf("if (e.metaKey && e.key === 'k') {")
    expect(at).toBeGreaterThan(-1)
    expect(app.slice(at, at + 240)).toContain('toggleCommandPalette()')
  })

  it('right-click Remove from Home drops the row only', async () => {
    menu.next = 'home-remove'
    menu.items = []
    render(<Shell />)
    await act(async () => {
      fireEvent.contextMenu(rowOf('Cee'))
    })
    expect(menu.items[0].map((i) => i.label)).toContain('Remove from Group 1')
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toEqual(['cortana::local', 'bee::b.k2.dev'])
    // The connected row's menu is the workspace menu, Pin (with the ⌘N it
    // will get) and remove.
    menu.next = null
    await act(async () => {
      fireEvent.contextMenu(rowOf('Cortana'))
    })
    expect(menu.items[1].map((i) => i.id)).toEqual(['home-settings', 'home-wiki', 'home-sep', 'home-pin', 'home-remove'])
  })

  it('the ⌘K field is the palette\'s entry: a click or typing opens it, the text carried over; it never filters', () => {
    render(<Shell />)
    const field = within(homeSidebar()).getByRole('textbox', { name: 'Search workspaces (⌘K)' }) as HTMLInputElement
    expect(field.placeholder).toBe('Search workspaces…')
    // Room for the ⌘K hint; a placeholder too long for the field ends in "…".
    expect(field.style.paddingRight).toBe('40px')
    expect(field.style.textOverflow).toBe('ellipsis')
    expect(field.getAttribute('aria-haspopup')).toBe('dialog')
    expect(field.getAttribute('aria-keyshortcuts')).toBe('Meta+K')
    expect(homeSidebar().querySelector('[data-home-palette-kbd]')?.textContent).toBe('⌘K')
    const rowsBefore = homeSidebar().querySelectorAll('[data-home-row]').length
    fireEvent.click(field)
    expect(useCommandPaletteStore.getState()).toMatchObject({ isOpen: true, prefill: '' })
    act(() => useCommandPaletteStore.getState().close())
    fireEvent.change(field, { target: { value: 'n' } })
    expect(useCommandPaletteStore.getState()).toMatchObject({ isOpen: true, prefill: 'n' })
    // The field itself keeps no text and filters nothing.
    expect(field.value).toBe('')
    expect(homeSidebar().querySelectorAll('[data-home-row]').length).toBe(rowsBefore)
    act(() => useCommandPaletteStore.getState().close())
    fireEvent.keyDown(field, { key: 'Enter' })
    expect(useCommandPaletteStore.getState().isOpen).toBe(true)
    act(() => useCommandPaletteStore.getState().close())
  })

  it('tab order: …, «, the list, the group bar, search, +', () => {
    render(<Shell />)
    const nav = homeSidebar()
    const order = [
      nav.querySelector('[data-group-edit-open]'),
      nav.querySelector('[data-group-name-header] [data-nav-collapse]'),
      nav.querySelector('[data-testid="home-nav-list"]'),
      nav.querySelector('[data-home-group-bar]'),
      nav.querySelector('[data-home-palette-input]'),
      nav.querySelector('[data-home-add-open="footer"]'),
    ]
    for (const el of order) expect(el).not.toBeNull()
    for (let i = 1; i < order.length; i++) {
      expect(order[i - 1]!.compareDocumentPosition(order[i]!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    }
  })

  it('the rail: « then … at the top; one search (the palette) then + at the bottom; + opens Add to Home', () => {
    useSidebarStore.setState({ isCollapsed: true })
    render(<Shell />)
    const rail = document.querySelector('[data-home-rail]') as HTMLElement
    const top = rail.querySelector('[data-home-rail-top]') as HTMLElement
    expect(Array.from(top.querySelectorAll('button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Open workspaces sidebar',
      'Edit groups (Group 1)',
    ])
    const bottom = rail.querySelector('[data-home-rail-bottom]') as HTMLElement
    expect(Array.from(bottom.querySelectorAll('button')).map((b) => b.getAttribute('aria-label'))).toEqual([
      'Search workspaces (⌘K)',
      'Add to Home',
    ])
    // One search entry only, below the group icons.
    expect(rail.querySelectorAll('[aria-label="Search workspaces (⌘K)"], [title="Search Workspaces (⌘K)"]').length).toBe(1)
    const bar = rail.querySelector('[data-home-group-bar]') as HTMLElement
    expect(bar.compareDocumentPosition(bottom) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    fireEvent.click(within(bottom).getByRole('button', { name: 'Search workspaces (⌘K)' }))
    expect(useCommandPaletteStore.getState().isOpen).toBe(true)
    act(() => useCommandPaletteStore.getState().close())
    // + opens Add to Home right here, without opening the nav.
    const plus = within(bottom).getByRole('button', { name: 'Add to Home' })
    fireEvent.click(plus)
    expect(screen.getByRole('dialog', { name: 'Add to Home' })).toBeTruthy()
    expect(useSidebarStore.getState().isCollapsed).toBe(true)
    fireEvent.click(screen.getByTestId('dialog-close'))
    expect(screen.queryByRole('dialog', { name: 'Add to Home' })).toBeNull()
    expect(document.activeElement).toBe(plus)
    // « opens the nav.
    fireEvent.click(within(top).getByRole('button', { name: 'Open workspaces sidebar' }))
    expect(useSidebarStore.getState().isCollapsed).toBe(false)
  })

  it('Add to Home is 560 × 540 at most, 16px from the sides and 24px from the top and bottom, a full sheet under 640px across', () => {
    expect(ADD_TO_HOME_MAX).toEqual({ width: 560, height: 540 })
    expect(ADD_TO_HOME_MARGIN).toEqual({ x: 16, y: 24 })
    expect(ADD_TO_HOME_SHEET_BELOW_PX).toBe(640)
    expect(ADD_TO_HOME_SIZE_CLASS.split(' ')).toEqual([
      `w-[min(${ADD_TO_HOME_MAX.width}px,calc(var(--k2-vw)-${2 * ADD_TO_HOME_MARGIN.x}px))]`,
      `h-[min(${ADD_TO_HOME_MAX.height}px,calc(var(--k2-vh)-${2 * ADD_TO_HOME_MARGIN.y}px))]`,
      `max-[${ADD_TO_HOME_SHEET_BELOW_PX + 2 * ADD_TO_HOME_MARGIN.x - 1}px]:w-[calc(var(--k2-vw)-${2 * ADD_TO_HOME_MARGIN.x}px)]`,
      `max-[${ADD_TO_HOME_SHEET_BELOW_PX + 2 * ADD_TO_HOME_MARGIN.x - 1}px]:h-[calc(var(--k2-vh)-${2 * ADD_TO_HOME_MARGIN.y}px)]`,
    ])
    render(<Shell />)
    fireEvent.click(within(homeSidebar()).getByRole('button', { name: 'Add to Home' }))
    const surface = screen.getByRole('dialog', { name: 'Add to Home' })
    for (const c of ADD_TO_HOME_SIZE_CLASS.split(' ')) expect(surface.className).toContain(c)
    // The list fades right under the search, heavy, in the dialog's fill.
    const frame = within(surface).getByTestId('add-to-home-list')
    expect(frame.hasAttribute('data-edge-heavy')).toBe(true)
    expect(frame.style.getPropertyValue('--k2-edge-tint')).toBe('var(--tile-fill, var(--color-bg-surface))')
    const header = frame.previousElementSibling as HTMLElement
    expect(header.hasAttribute('data-edge-header')).toBe(true)
    expect(header.className).not.toMatch(/(^|\s)(pb|py)-/)
  })

  it('Add to Home lists each saved server in its own section, minus the connected one (Q3)', async () => {
    render(<Shell />)
    fireEvent.click(within(homeSidebar()).getByRole('button', { name: 'Add to Home' }))
    const dialog = screen.getByRole('dialog', { name: 'Add to Home' })
    const groups = within(dialog).getAllByRole('group')
    expect(groups.map((g) => g.getAttribute('aria-label'))).toEqual(['This computer', 'Box B', 'Box C'])
    // B lists with its own login; Bee is already on this Home.
    await waitFor(() => expect(within(groups[1]).getAllByRole('option').length).toBe(2))
    const bee = within(groups[1]).getByRole('option', { name: /Bee/ })
    expect(bee.getAttribute('aria-disabled')).toBe('true')
    // C has no login: it asks for one.
    expect(groups[2].querySelector('[data-add-source-status]')?.getAttribute('data-add-source-status')).toBe('signin')
    fireEvent.click(within(groups[1]).getByRole('option', { name: /Quill/ }))
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toContain('quill::b.k2.dev')
    expect(screen.queryByRole('dialog', { name: 'Add to Home' })).toBeNull()
    // No request for C's agents: it has no login.
    expect(fetchMock.mock.calls.map((c) => c[0]).filter((u) => u.startsWith('https://c.k2.dev/cli/'))).toEqual([])
  })

})

// Rosson 2026-10-30: an agent on another server keeps its own color (the
// machine chip already says where it lives). The color is data from that
// server's listing, validated as `#rrggbb`; unknown → the muted color;
// offline / not-found rows keep their dimming.
describe('the color of a row on another server', () => {
  const MUTED = 'var(--color-text-muted)'

  /** The letter avatar's fill (ProjectAvatar's letter square). */
  function fillOf(el: Element | null): string {
    const letter = el?.querySelector('span > span[style*="background-color"]')
    if (!(letter instanceof HTMLElement)) throw new Error('no letter avatar in this row')
    return letter.style.backgroundColor
  }

  function railRow(address: string): HTMLElement {
    const el = document.querySelector(`[data-home-rail-row="${address}"]`)
    if (!(el instanceof HTMLElement)) throw new Error(`no rail icon for ${address}`)
    return el
  }

  beforeEach(() => {
    // Dee's server is not saved on this computer: its row is offline.
    useHomesStore.getState().addRow(useHomesStore.getState().homes[0].id, {
      address: 'dee::d.k2.dev',
      workspaceId: 'pd',
      label: 'Dee',
    })
    // What earlier listings recorded: Bee and Dee; Cee was never seen.
    localStorage.setItem(
      HOME_AGENT_COLORS_KEY,
      JSON.stringify({ version: 1, colors: { 'bee::b.k2.dev': '#123456', 'dee::d.k2.dev': '#AbC' } }),
    )
  })

  it('sidebar: its agent color when known, muted when unknown, dimmed when offline', async () => {
    render(<Shell />)
    await waitFor(() => expect(fillOf(rowOf('Bee'))).toBe('rgb(18, 52, 86)'))
    expect(fillOf(rowOf('Cee'))).toBe(MUTED)
    // Offline keeps its own color and its dimming.
    expect(rowOf('Dee').querySelector('[data-row-status]')?.getAttribute('data-row-status')).toBe('offline')
    expect(fillOf(rowOf('Dee'))).toBe('rgb(170, 187, 204)')
    expect(rowOf('Dee').className).toContain('opacity-60')
    expect(rowOf('Bee').className).not.toContain('opacity-60')
  })

  it('icon rail: the same colors, and the offline icon stays dimmed', async () => {
    useSidebarStore.setState({ isCollapsed: true })
    render(<Shell />)
    await waitFor(() => expect(fillOf(railRow('bee::b.k2.dev'))).toBe('rgb(18, 52, 86)'))
    expect(fillOf(railRow('cee::c.k2.dev'))).toBe(MUTED)
    expect(railRow('dee::d.k2.dev').getAttribute('data-row-status')).toBe('offline')
    expect(fillOf(railRow('dee::d.k2.dev'))).toBe('rgb(170, 187, 204)')
    expect(railRow('dee::d.k2.dev').className).toContain('opacity-60')
    expect(railRow('bee::b.k2.dev').className).not.toContain('opacity-60')
  })

  it('a stored color that is not hex never reaches a style: the row paints muted', async () => {
    localStorage.setItem(
      HOME_AGENT_COLORS_KEY,
      JSON.stringify({
        version: 1,
        colors: {
          'bee::b.k2.dev': 'red; background-image: url(https://evil.example/x)',
          'cee::c.k2.dev': 'var(--x)',
          'dee::d.k2.dev': '#abc',
        },
      }),
    )
    render(<Shell />)
    await waitFor(() => expect(fillOf(rowOf('Dee'))).toBe('rgb(170, 187, 204)'))
    expect(fillOf(rowOf('Bee'))).toBe(MUTED)
    expect(fillOf(rowOf('Cee'))).toBe(MUTED)
    expect(rowOf('Bee').innerHTML).not.toContain('evil.example')
  })

  it('Add to Home paints each agent from a server in its listed color, and the added row keeps it', async () => {
    render(<Shell />)
    fireEvent.click(within(homeSidebar()).getByRole('button', { name: 'Add to Home' }))
    const dialog = screen.getByRole('dialog', { name: 'Add to Home' })
    const box = within(dialog).getByRole('group', { name: 'Box B' })
    await waitFor(() => expect(within(box).getAllByRole('option').length).toBe(2))
    expect(fillOf(within(box).getByRole('option', { name: /Quill/ }))).toBe('rgb(101, 67, 33)')
    fireEvent.click(within(box).getByRole('option', { name: /Quill/ }))
    await waitFor(() => expect(useHomeAvatarStore.getState().colors['quill::b.k2.dev']).toBe('#654321'))
    const quillRow = Array.from(homeSidebar().querySelectorAll('[data-agent-row]')).find((r) =>
      r.textContent?.includes('Quill'),
    )
    expect(fillOf(quillRow ?? null)).toBe('rgb(101, 67, 33)')
  })

})

// Rosson 2026-10-16: "When going to home, if there is no agent selected,
// select the top-most one." Each mount of HomeShellEffects is an entry.
describe('entering Home selects the top row when nothing is selected', () => {
  // Homes made here go, so later tests see the one Home.
  afterEach(() => {
    const st = useHomesStore.getState()
    for (const h of st.homes.slice(1)) st.deleteHome(h.id)
  })

  /** A second Home with these rows; the first stays selected. */
  function addWorkHome(rows: { address: string; workspaceId: string; label: string }[]): string {
    const st = useHomesStore.getState()
    const first = st.homes[0].id
    const id = st.createHome('Work')
    if (!id) throw new Error('createHome refused')
    for (const r of rows) useHomesStore.getState().addRow(id, r)
    useHomesStore.getState().selectHome(first)
    return id
  }

  it('picking another Home with no valid selection on it opens that Home\'s top row', () => {
    const work = addWorkHome([{ address: 'nova::local', workspaceId: 'pn', label: 'Nova' }])
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    act(() => useHomesStore.getState().selectHome(work))
    expect(useProjectsStore.getState().activeProjectId).toBe('pn')
    expect(rowOf('Nova').getAttribute('aria-current')).toBe('true')
  })

  it('picking another Home keeps a selection that is a row of it', () => {
    const work = addWorkHome([
      { address: 'nova::local', workspaceId: 'pn', label: 'Nova' },
      { address: 'cortana::local', workspaceId: 'pl', label: 'Cortana' },
    ])
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    act(() => useHomesStore.getState().selectHome(work))
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    expect(rowOf('Cortana').getAttribute('aria-current')).toBe('true')
  })

  it('picking an empty Home selects nothing new', () => {
    const work = addWorkHome([])
    render(<Shell />)
    act(() => useHomesStore.getState().selectHome(work))
    expect(useHomesStore.getState().homes.find((h) => h.id === work)?.rows).toEqual([])
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
  })

  it('no selection: the top row opens, as a click on it would', () => {
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    expect(useProjectsStore.getState().activeWorkspaceId).toBe('wl')
    expect(rowOf('Cortana').getAttribute('aria-current')).toBe('true')
    expect(within(roomArea()).getByTestId('terminal-area').getAttribute('data-cwd')).toBe('/w/cortana')
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('a selection that is not on this Home (stale) gives way to the top row', () => {
    useProjectsStore.getState().setActiveProject('pn')
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    expect(rowOf('Cortana').getAttribute('aria-current')).toBe('true')
  })

  it('a selected row of this Home is kept, even when it is not the top one', () => {
    const home = useHomesStore.getState().homes[0]
    useHomesStore.getState().addRow(home.id, { address: 'nova::local', workspaceId: 'pn', label: 'Nova' })
    useProjectsStore.getState().setActiveProject('pn')
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBe('pn')
    expect(rowOf('Nova').getAttribute('aria-current')).toBe('true')
    expect(rowOf('Cortana').getAttribute('aria-current')).toBeNull()
  })

  it('an empty Home selects nothing', () => {
    const home = useHomesStore.getState().homes[0]
    for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
  })

  it('on a window opening before the server list lands, it waits and opens the top row once it does', () => {
    const loaded = useProjectsStore.getState().projects
    useProjectsStore.setState({ projects: [] })
    render(<Shell />)
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
    act(() => useProjectsStore.setState({ projects: loaded }))
    expect(useProjectsStore.getState().activeProjectId).toBe('pl')
    // Once settled, a later change of selection is the user's: no re-pick.
    act(() => useProjectsStore.setState({ activeProjectId: null, activeWorkspaceId: null }))
    act(() => useProjectsStore.setState({ projects: [...loaded] }))
    expect(useProjectsStore.getState().activeProjectId).toBeNull()
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
