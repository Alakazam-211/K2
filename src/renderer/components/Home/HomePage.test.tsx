// @vitest-environment jsdom
//
// Home P1 — the page wired end to end in jsdom: a connected-server row
// shows its activity, a row on another saved server polls that server's
// public /boot-status and its signed-in presence summary (with THAT
// server's token) and shows Live + who is there, a server with no login
// shows "Sign in" without being asked, and clicking a remote row switches
// through pickHost with a pending select. Top-bar chrome is stubbed; the
// homes, page-view, connect-host stores and the status code are real.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/components/TopBar/ServerSwitcher', () => ({
  default: () => null,
  hostDisplayAddress: (h: { hostname: string; port: number; secure: boolean }) =>
    h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`,
}))
vi.mock('@/components/TopBar/PageTabs', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({ default: () => null }))
vi.mock('@/components/TopBar/K2MarkButton', () => ({ default: () => null }))
vi.mock('@/lib/titlebar-drag', () => ({
  titleBarDragOnMouseDown: () => undefined,
  titleBarOnDoubleClick: () => undefined,
}))

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
  const projects = hookOf({
    projects: [{ id: 'pl', name: 'Cortana', path: '/w/cortana', handle: 'cortana', workspaces: [] }],
    setActiveProject: (() => undefined) as (id: string) => void,
  })
  const activity = hookOf({ getProjectStatus: (id: string) => (id === 'pl' ? 'working' : 'idle') })
  const presence = hookOf({ roster: [] as unknown[], supported: true })
  return { hookOf, projects, activity, presence }
})
vi.mock('@/stores/projects', () => ({ useProjectsStore: h.projects }))
vi.mock('@/stores/active-agents', () => ({ useActiveAgentsStore: h.activity }))
vi.mock('@/stores/presence', async () => {
  const real = await vi.importActual<typeof import('@/stores/presence')>('@/stores/presence')
  return { usePresenceStore: h.presence, usersForWorkspace: real.usersForWorkspace }
})
vi.mock('@/stores/session-events', () => ({
  onAppHello: vi.fn(),
  onPresenceChanged: vi.fn(),
}))

import HomePage from './HomePage'
import { useHomesStore } from '@/stores/homes'
import { usePageViewStore } from '@/stores/page-view'
import { useConnectHostStore, __resetConnectHostStoreForTests, type ConnectHost } from '@/stores/connect-host'
import { useHomeProbeStore } from '@/lib/home-status'
import { peekHostSelect, clearHostSelect } from '@/lib/home-pending-select'

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

beforeEach(() => {
  vi.stubGlobal('fetch', fetchMock)
  __resetConnectHostStoreForTests()
  useConnectHostStore.setState({ hosts: [boxB, boxC], connectionStatus: 'connected' })
  clearHostSelect()
  useHomeProbeStore.setState({ probes: {} })
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
  vi.unstubAllGlobals()
})

function rowOf(label: string): HTMLElement {
  const el = screen.getByText(label).closest('li')
  if (!el) throw new Error(`no row for ${label}`)
  return el
}

describe('HomePage', () => {
  it('renders nothing unless the Home page is selected', () => {
    usePageViewStore.getState().setPage('agents')
    const { container } = render(<HomePage />)
    expect(container.innerHTML).toBe('')
  })

  it('connected row = activity; other server = Live + presence; no login = Sign in', async () => {
    render(<HomePage />)
    expect(rowOf('Cortana').textContent).toContain('Working')
    expect(rowOf('Cortana').textContent).toContain('This computer')
    expect(rowOf('Cee').textContent).toContain('Sign in')

    await waitFor(() => expect(rowOf('Bee').textContent).toContain('Live'))
    // Presence: anna is there; you (rosson on B) are not shown.
    expect(screen.getByLabelText('Here now: anna')).toBeTruthy()
    expect(screen.queryByLabelText(/rosson/)).toBeNull()

    const urls = fetchMock.mock.calls.map((c) => c[0])
    expect(urls).toContain('https://b.k2.dev/boot-status')
    expect(urls).toContain('https://b.k2.dev/cli/presence/summary?token=tok-b')
    // A server with no login is never asked.
    expect(urls.some((u) => u.includes('c.k2.dev'))).toBe(false)
  })

  it('clicking a row on another server switches through pickHost with a pending select', async () => {
    const pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost })
    render(<HomePage />)
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
  })

  it('clicking a row on the connected server selects it and goes to Agents', () => {
    const setActiveProject = vi.fn()
    h.projects.setState({ setActiveProject })
    render(<HomePage />)
    fireEvent.click(rowOf('Cortana'))
    expect(setActiveProject).toHaveBeenCalledWith('pl')
    expect(usePageViewStore.getState().page).toBe('agents')
  })

  it('remove drops the row only', () => {
    render(<HomePage />)
    fireEvent.click(screen.getByLabelText('Remove Cee from Home'))
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toEqual(['cortana::local', 'bee::b.k2.dev'])
  })

  it('Add Agent lists only this server’s agents not already on the Home', () => {
    h.projects.setState({
      projects: [
        { id: 'pl', name: 'Cortana', path: '/w/cortana', handle: 'cortana', workspaces: [] },
        { id: 'pn', name: 'Nova', path: '/w/nova', handle: 'nova', workspaces: [] },
      ],
    })
    render(<HomePage />)
    fireEvent.click(screen.getByText('Add Agent'))
    const menu = screen.getByRole('menu')
    expect(menu.textContent).toContain('Agents on This computer')
    expect(menu.textContent).toContain('Nova')
    expect(menu.textContent).not.toContain('Cortana')
    fireEvent.click(screen.getByText('Nova'))
    expect(useHomesStore.getState().homes[0].rows.map((r) => r.address)).toContain('nova::local')
  })
})
