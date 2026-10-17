// @vitest-environment jsdom
// 0.46.0 (Rosson 2026-10-16): the Workspace tab no longer shows the agent's
// display name or handle (they live on the Agent tab). Its header puts View
// Wiki on the left and, on the right, the Thread push phone toggle beside
// the completion bell. A room on another server offers View Wiki too: it
// switches the window to that server, then opens the Wiki there.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => 0) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn((_s: unknown, route: string) => {
    // A window switch re-fetches the window's list; the tests land it
    // themselves (`useProjectsStore.setState`), so this one never settles.
    if (route === 'projects/list') return new Promise(() => undefined)
    return Promise.reject(new Error(`unexpected GET ${route}`))
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))
vi.mock('@/components/HeartbeatsPanel/HeartbeatsPanel', () => ({ HeartbeatsPanel: () => null }))
vi.mock('./ConnectedAgentsSection', () => ({ ConnectedAgentsSection: () => null }))
vi.mock('./UrlsPortsSection', () => ({ UrlsPortsSection: () => null }))
vi.mock('./WorkspaceApiSection', () => ({ WorkspaceApiSection: () => null }))
vi.mock('./WorkspaceCompletionSoundToggle', () => ({
  WorkspaceCompletionSoundBell: () => <button type="button" data-testid="completion-bell">bell</button>,
}))
vi.mock('./WorkspacePhonePushToggle', () => ({
  WorkspacePhonePushBell: ({ project }: { project: { id: string } }) => (
    <button type="button" data-testid="phone-push-bell" data-project={project.id}>phone</button>
  ),
}))

import WorkspacePanel from './WorkspacePanel'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { usePageViewStore } from '@/stores/page-view'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { clearHostSelect, peekHostSelect } from '@/lib/home-pending-select'
import { __cancelRoomWikiOpenForTests } from '@/lib/room-wiki-open'
import type { ServerScope } from '@/kessel/server-scope'

const project = {
  id: 'p1',
  name: 'cortana',
  path: '/ws/cortana',
  color: '#3b82f6',
  iconUrl: null,
  handle: 'cortana-handle',
  agentMode: 'custom',
  workspaces: [],
} as unknown as ProjectWithWorkspaces

const hostB = {
  id: 'h-b',
  label: 'B box',
  hostname: 'b.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-b',
} as unknown as ConnectHost

function render(isPrimary: boolean, scope?: ServerScope): void {
  const tabs = createStore(() => ({ openAgentPane: vi.fn() }))
  const room = testRoom({
    tabs,
    isPrimary,
    scope: isPrimary ? undefined : scope ?? fakeScope('b.k2.dev'),
    localCommands: isPrimary,
    projects: [project],
    activeProjectId: 'p1',
    key: isPrimary ? 'primary' : 'b',
  })
  renderInRoom(room, <WorkspacePanel />)
}

const realPickHost = useConnectHostStore.getState().pickHost

beforeEach(() => {
  useConnectHostStore.setState({ activeHost: 'local', hosts: [hostB], connectionStatus: 'connected', pendingSignIn: null })
})

afterEach(() => {
  cleanup()
  __cancelRoomWikiOpenForTests()
  clearHostSelect()
  useConnectHostStore.setState({
    activeHost: 'local',
    hosts: [],
    pickHost: realPickHost,
    pendingSignIn: null,
    recovery: { kind: 'connected' },
  })
  useProjectsStore.setState({ projects: [] })
  usePageViewStore.getState().setPage('agents')
})

/** A pickHost that switches silently (a saved token), as `selectHost` does. */
function silentSwitch(): ReturnType<typeof vi.fn> {
  const pickHost = vi.fn((h: 'local' | ConnectHost) => {
    useConnectHostStore.setState({ activeHost: h, connectionStatus: 'connecting' })
  })
  useConnectHostStore.setState({ pickHost })
  return pickHost
}

describe('Workspace tab header', () => {
  it('shows no display name or handle', () => {
    render(true)
    expect(screen.queryByText('Display Name')).toBeNull()
    expect(screen.queryByText('Handle')).toBeNull()
    expect(screen.queryByText('cortana-handle')).toBeNull()
  })

  it('puts View Wiki on the left and the phone toggle beside the bell on the right', () => {
    render(true)
    const header = screen.getByTestId('workspace-panel-header')
    const kids = Array.from(header.children)
    expect(kids).toHaveLength(2)
    expect(kids[0].textContent).toBe('View Wiki')
    expect(kids[1]).toBe(screen.getByTestId('workspace-panel-bell'))
    expect(kids[1].className).toContain('ml-auto')
    expect(Array.from(kids[1].children)).toEqual([
      screen.getByTestId('phone-push-bell'),
      screen.getByTestId('completion-bell'),
    ])
    expect(screen.getByTestId('phone-push-bell').dataset.project).toBe('p1')
    expect(header.className).not.toContain('justify-end')
  })

  it('View Wiki opens this workspace\'s wiki directly; close returns to the page (0.46.0)', () => {
    usePageViewStore.getState().setPage('home')
    render(true)
    fireEvent.click(screen.getByText('View Wiki'))
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage]).toEqual(['wiki', '/ws/cortana', 'home'])
    s.closeWiki()
    expect(usePageViewStore.getState().page).toBe('home')
    usePageViewStore.getState().setPage('agents')
  })

  it('a room on another server keeps the phone toggle and the bell and offers View Wiki that switches servers', () => {
    render(false)
    const header = screen.getByTestId('workspace-panel-header')
    const kids = Array.from(header.children)
    expect(kids).toHaveLength(2)
    expect(kids[0].textContent).toBe('View Wiki')
    expect(kids[0].getAttribute('title')).toBe(
      'Switch this window to b.k2.dev and open the workspace knowledge base graph',
    )
    expect(kids[1]).toBe(screen.getByTestId('workspace-panel-bell'))
    expect(kids[1].className).toContain('ml-auto')
    expect(Array.from(kids[1].children)).toEqual([
      screen.getByTestId('phone-push-bell'),
      screen.getByTestId('completion-bell'),
    ])
  })

  it('View Wiki in a remote room switches the window, then opens that workspace\'s Wiki once ready', () => {
    const pickHost = silentSwitch()
    usePageViewStore.getState().setPage('home')
    render(false)
    fireEvent.click(screen.getByText('View Wiki'))
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost).toHaveBeenCalledWith(hostB)
    // The window's own room lands on the same workspace (Home's switch path).
    expect(peekHostSelect()).toMatchObject({
      hostId: 'h-b',
      row: { address: 'cortana-handle::b.k2.dev', workspaceId: 'p1' },
    })
    // Not yet: connecting, then connected before B's list lands.
    expect(usePageViewStore.getState().page).toBe('home')
    useConnectHostStore.setState({ connectionStatus: 'connected' })
    expect(usePageViewStore.getState().page).toBe('home')
    // B's list lands: the Wiki opens on that workspace (matched by id).
    useProjectsStore.setState({ projects: [project] })
    const s = usePageViewStore.getState()
    expect([s.page, s.wikiProjectPath, s.wikiReturnPage]).toEqual(['wiki', '/ws/cortana', 'home'])
  })

  it('a cancelled sign-in on the switch stays put and never opens the Wiki later', () => {
    const pickHost = vi.fn((h: 'local' | ConnectHost) => {
      useConnectHostStore.setState({ pendingSignIn: h as ConnectHost })
    })
    useConnectHostStore.setState({ pickHost })
    usePageViewStore.getState().setPage('home')
    render(false)
    fireEvent.click(screen.getByText('View Wiki'))
    expect(pickHost).toHaveBeenCalledWith(hostB)
    useConnectHostStore.setState({ pendingSignIn: null })
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(usePageViewStore.getState().page).toBe('home')
    // A later, unrelated switch to B does not open the Wiki.
    useConnectHostStore.setState({ activeHost: hostB, connectionStatus: 'connected' })
    useProjectsStore.setState({ projects: [project] })
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('a switch that lands on a server asking for a new sign-in stays put', () => {
    silentSwitch()
    usePageViewStore.getState().setPage('home')
    render(false)
    fireEvent.click(screen.getByText('View Wiki'))
    useConnectHostStore.setState({ recovery: { kind: 'signin-required' } })
    useConnectHostStore.setState({ recovery: { kind: 'connected' }, connectionStatus: 'connected' })
    useProjectsStore.setState({ projects: [project] })
    expect(usePageViewStore.getState().page).toBe('home')
  })

  it('a room on a server too old for the Wiki routes hides View Wiki', () => {
    const old: ServerScope = { ...fakeScope('b.k2.dev'), serverSupports: (f) => f !== 'wiki' }
    render(false, old)
    expect(screen.queryByText('View Wiki')).toBeNull()
    expect(Array.from(screen.getByTestId('workspace-panel-header').children)).toEqual([
      screen.getByTestId('workspace-panel-bell'),
    ])
  })
})
