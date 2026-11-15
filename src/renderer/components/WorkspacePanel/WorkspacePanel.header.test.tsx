// @vitest-environment jsdom
// 0.46.0 (Rosson 2026-10-16): the Workspace tab no longer shows the agent's
// display name or handle (they live on the Agent tab). Its header puts View
// Wiki on the left and, on the right, the Thread push phone toggle beside
// the completion bell. A room on another server offers View Wiki too: it
// switches the window to that server, then opens the Wiki there.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, screen } from '@testing-library/react'
import { invoke } from '@tauri-apps/api/core'
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

function render(isPrimary: boolean, scope?: ServerScope, shown: ProjectWithWorkspaces = project): void {
  const tabs = createStore(() => ({ openAgentPane: vi.fn() }))
  const room = testRoom({
    tabs,
    isPrimary,
    scope: isPrimary ? undefined : scope ?? fakeScope('b.k2.dev'),
    localCommands: isPrimary,
    projects: [shown],
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

// Rosson 2026-11-15: the drawer's header glyphs are K2's own icons (style
// A3: 24 grid, K2 stroke, square caps), not the old hand-drawn ones.
describe('Workspace tab header icons', () => {
  it('View Wiki uses WikiIcon in the accent colour at the header-glyph size', () => {
    render(true)
    const button = screen.getByText('View Wiki').closest('button') as HTMLElement
    const icon = button.querySelector('svg[data-k2-icon="wiki"]')
    expect(icon).not.toBeNull()
    expect(icon?.getAttribute('stroke-linecap')).toBe('square')
    expect(icon?.getAttribute('width')).toBe('12')
    expect(icon?.getAttribute('class')).toContain('text-[var(--color-accent)]')
    expect(button.querySelectorAll('svg')).toHaveLength(1)
    expect(button.querySelector('circle')).toBeNull()
  })

  it('View Inbox uses InboxIcon, not a rounded square', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'k2_inbox_count' ? 2 : 0))
    render(true, undefined, { ...project, agentMode: 'off' } as unknown as ProjectWithWorkspaces)
    const label = await screen.findByText('View Inbox')
    const row = label.parentElement as HTMLElement
    const icon = row.querySelector('svg[data-k2-icon="inbox"]')
    expect(icon).not.toBeNull()
    expect(icon?.getAttribute('width')).toBe('12')
    expect(icon?.getAttribute('class')).toContain('text-[var(--color-accent)]')
    expect(row.querySelector('rect[rx]')).toBeNull()
    vi.mocked(invoke).mockImplementation(async () => 0)
  })
})

// Rosson 2026-11-15: View Wiki and View Inbox sat at different insets (px-2
// in a px-3 block; px-2 with -mx-1), off the section headers' chevrons. Both
// now span the drawer (full-width hover) with the drawer's one left inset,
// px-3, and the same icon-to-text gap, so their icons and text line up with
// each other and with the chevrons below.
describe('Workspace tab Status row inset', () => {
  it('View Wiki and View Inbox share the drawer inset (px-3) and gap; the block adds none', async () => {
    vi.mocked(invoke).mockImplementation(async (cmd: string) => (cmd === 'k2_inbox_count' ? 2 : 0))
    render(true, undefined, { ...project, agentMode: 'off' } as unknown as ProjectWithWorkspaces)
    const wiki = screen.getByText('View Wiki').closest('button') as HTMLElement
    // The clickable row (its hover band).
    const inbox = (await screen.findByText('View Inbox')).closest('.cursor-pointer') as HTMLElement
    expect(inbox).not.toBeNull()
    for (const el of [wiki, inbox]) {
      const cls = el.className.split(/\s+/)
      expect({ px: cls.filter((c) => /^-?(p|m)[xl]-/.test(c)), gap: cls.filter((c) => c.startsWith('gap-')) })
        .toEqual({ px: ['px-3'], gap: ['gap-1.5'] })
    }
    // The block spans the drawer: no horizontal padding of its own.
    const block = screen.getByTestId('workspace-panel-header').parentElement as HTMLElement
    expect(block.className.split(/\s+/).filter((c) => /^p[xl]-/.test(c))).toEqual([])
    // The bells keep the drawer's right inset.
    expect(screen.getByTestId('workspace-panel-header').className.split(/\s+/)).toContain('pr-3')
    vi.mocked(invoke).mockImplementation(async () => 0)
  })
})
