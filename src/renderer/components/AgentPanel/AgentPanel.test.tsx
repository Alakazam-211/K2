// @vitest-environment jsdom
// The Agent drawer tab (0.46.0, Rosson 2026-10-16): profile image, color,
// agent name + handle, permission switches — live, through the same
// controls and routes as Settings → Workspaces → Agent. Editable in the
// window's own room; read-only for a room on another server; switches
// follow the route floors for the viewer's role.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>
type Call = { host: string; route: string; body: Json }

const cli = vi.hoisted(() => ({
  names: {} as Record<string, string>,
  handles: {} as Record<string, string>,
  gets: [] as Call[],
  posts: [] as Call[],
}))
const identity = vi.hoisted(() => ({ handlers: [] as Array<{ host: string; fn: (e: unknown) => void }> }))
const role = vi.hoisted(() => ({ value: 'owner' as 'owner' | 'admin' | 'member' | null }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

vi.mock('@/lib/daemon-cli', async (orig) => {
  const real = await orig<typeof import('@/lib/daemon-cli')>()
  const answer = (route: string, host: string): unknown => {
    if (route === 'workspace/agent-display-name') return { display_name: cli.names[host] }
    if (route === 'workspace/handle') return { handle: cli.handles[host] }
    if (route === 'workspace/set-handle') return { handle: 'cortana-prime' }
    return { ok: true }
  }
  return {
    ...real,
    daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, body: Json = {}) => {
      cli.gets.push({ host: scope.hostKey, route, body })
      if (route !== 'workspace/agent-display-name' && route !== 'workspace/handle') {
        throw new Error(`unexpected GET ${route}`)
      }
      return answer(route, scope.hostKey)
    }),
    daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body: Json = {}) => {
      cli.posts.push({ host: scope.hostKey, route, body })
      return answer(route, scope.hostKey)
    }),
    daemonCliPostQuery: vi.fn(async (scope: { hostKey: string }, route: string, body: Json = {}) => {
      cli.posts.push({ host: scope.hostKey, route, body })
      return answer(route, scope.hostKey)
    }),
  }
})

vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onAgentIdentityChanged: (scope: { hostKey: string }, fn: (e: unknown) => void) => {
    const entry = { host: scope.hostKey, fn }
    identity.handlers.push(entry)
    return () => {
      identity.handlers = identity.handlers.filter((h) => h !== entry)
    }
  },
}))

vi.mock('@/components/Presence/PresenceKickButton', () => ({
  fetchViewerRole: vi.fn(async () => role.value),
}))

vi.mock('@/hooks/useTunnelUrls', () => ({
  useTunnelUrls: () => ({ status: { running: true, public_url: 'https://rosson.k2.dev' } }),
}))

import AgentPanel from './AgentPanel'
import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { primaryScope } from '@/kessel/server-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import type { RoomProjectsStore } from '@/stores/room'

const PRIMARY_HOST = primaryScope().hostKey

function project(over: Partial<ProjectWithWorkspaces> = {}): ProjectWithWorkspaces {
  return {
    id: 'p1',
    name: 'cortana',
    path: '/ws/cortana',
    color: '#3b82f6',
    iconUrl: null,
    handle: 'cortana',
    workspaces: [],
    agentsCanManageAgents: 0,
    agentsCanManageVault: 0,
    agentsCanUseCompute: 0,
    agentsCanOperateCompute: 0,
    agentsCanManageProjects: 1,
    agentsCanManageSkin: 0,
    agentsCanCreateConnections: 0,
    allowRemoteInstruct: 0,
    dnsManageEnabled: 0,
    mailManageEnabled: 0,
    dbAgentAccess: 'off',
    ...over,
  } as ProjectWithWorkspaces
}

const fetchProjects = vi.fn(async () => {})

function renderPrimary(p: ProjectWithWorkspaces = project()): void {
  useProjectsStore.setState({ projects: [p], fetchProjects })
  const room = testRoom({
    tabs: {},
    isPrimary: true,
    projects: useProjectsStore as unknown as RoomProjectsStore,
    activeProjectId: p.id,
  })
  renderInRoom(room, <AgentPanel />)
}

function sw(id: string): HTMLButtonElement {
  const el = document.querySelector(`[data-settings-id="${id}"]`)
  if (!(el instanceof HTMLButtonElement)) throw new Error(`no switch ${id}`)
  return el
}

const ADMIN_SWITCHES = [
  'projects.allow-remote-instruct',
  'projects.dns-manage-enabled',
  'projects.mail-manage-enabled',
  'projects.agents-can-manage-agents',
  'projects.agents-can-manage-vault',
  'projects.agents-can-use-compute',
  'projects.agents-can-manage-projects',
  'projects.agents-can-create-connections',
]
const ALL_SWITCHES = [
  ...ADMIN_SWITCHES,
  'projects.db-agent-create',
  'projects.agents-can-manage-skin',
  'projects.agents-can-operate-compute',
]

beforeEach(() => {
  cli.names = { [PRIMARY_HOST]: 'Cortana', 'b.k2.dev': 'Baden Bot' }
  cli.handles = { [PRIMARY_HOST]: 'cortana', 'b.k2.dev': 'baden' }
  cli.gets = []
  cli.posts = []
  identity.handlers = []
  role.value = 'owner'
  fetchProjects.mockClear()
})

afterEach(() => {
  cleanup()
})

describe('Agent tab: a local agent (the window’s room)', () => {
  it('shows the four sections in Rosson’s order, filled from the daemon', async () => {
    renderPrimary()
    const sections = Array.from(document.querySelectorAll('section')).map((s) => s.getAttribute('aria-label'))
    expect(sections).toEqual(['Profile image', 'Color', 'Agent name and handle', 'Permissions'])

    await waitFor(() => expect((screen.getByLabelText('Agent Name') as HTMLInputElement).value).toBe('Cortana'))
    await waitFor(() => expect((screen.getByLabelText('Handle') as HTMLInputElement).value).toBe('cortana'))
    expect(screen.getByText('cortana::rosson.k2.dev')).toBeTruthy()
    expect(screen.getByTestId('agent-picture-tile')).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Detect' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Upload' })).toBeTruthy()
    expect(screen.queryByTestId('agent-panel-readonly-note')).toBeNull()

    await waitFor(() => expect(document.querySelector('[data-settings-id="projects.agents-can-operate-compute"]')).not.toBeNull())
    for (const id of ALL_SWITCHES) {
      expect([id, sw(id).disabled]).toEqual([id, false])
    }
    // Compact rows: the long Settings descriptions are hidden in the drawer.
    expect(screen.getByText('Allow hiring and managing agents')).toBeTruthy()
    const desc = screen.getByText(/Lets this workspace’s main agent open and stop sidecars/)
    expect(desc.className).toContain('hidden')
  })

  it('reads name and handle from the room’s server', async () => {
    renderPrimary()
    await waitFor(() => expect(cli.gets.map((g) => g.route)).toContain('workspace/handle'))
    expect(cli.gets.every((g) => g.host === PRIMARY_HOST && g.body.project === '/ws/cortana')).toBe(true)
  })

  it('a rename saves through workspace/set-agent-display-name', async () => {
    renderPrimary()
    const input = screen.getByLabelText('Agent Name') as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('Cortana'))
    fireEvent.change(input, { target: { value: 'Cortana Prime' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    await waitFor(() =>
      expect(cli.posts).toContainEqual({
        host: PRIMARY_HOST,
        route: 'workspace/set-agent-display-name',
        body: { project: '/ws/cortana', name: 'Cortana Prime' },
      }),
    )
  })

  it('an invalid name shows the shared validation copy and sends nothing', async () => {
    renderPrimary()
    const input = screen.getByLabelText('Agent Name') as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('Cortana'))
    fireEvent.change(input, { target: { value: 'a/b' } })
    fireEvent.click(screen.getByRole('button', { name: 'Save' }))
    expect(screen.getByText("Display name must not contain '/'.")).toBeTruthy()
    expect(cli.posts).toEqual([])
  })

  it('a handle change asks first, then saves through workspace/set-handle', async () => {
    const confirm = vi.fn(async (_opts: unknown) => true)
    useConfirmDialogStore.setState({ confirm })
    renderPrimary()
    const input = screen.getByLabelText('Handle') as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('cortana'))
    fireEvent.change(input, { target: { value: 'cortana-prime' } })
    fireEvent.click(screen.getByRole('button', { name: 'Change…' }))
    await waitFor(() =>
      expect(cli.posts).toContainEqual({
        host: PRIMARY_HOST,
        route: 'workspace/set-handle',
        body: { project: '/ws/cortana', handle: 'cortana-prime' },
      }),
    )
    expect(confirm).toHaveBeenCalledTimes(1)
    expect(confirm.mock.calls[0][0]).toMatchObject({ title: 'Change handle?', confirmLabel: 'Change handle' })
  })

  it('a cancelled handle change sends nothing', async () => {
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => false) })
    renderPrimary()
    const input = screen.getByLabelText('Handle') as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('cortana'))
    fireEvent.change(input, { target: { value: 'other' } })
    fireEvent.click(screen.getByRole('button', { name: 'Change…' }))
    await act(async () => {})
    expect(cli.posts.filter((p) => p.route === 'workspace/set-handle')).toEqual([])
  })

  it('a color swatch paints at once and saves through projects/update', async () => {
    renderPrimary()
    fireEvent.click(screen.getByRole('button', { name: 'Color #ef4444' }))
    expect(useProjectsStore.getState().projects[0].color).toBe('#ef4444')
    await waitFor(() =>
      expect(cli.posts).toContainEqual({ host: PRIMARY_HOST, route: 'projects/update', body: { id: 'p1', color: '#ef4444' } }),
    )
  })

  it('Remove clears the picture through projects/clear-icon', async () => {
    renderPrimary(project({ iconUrl: 'data:image/png;base64,AAAA' }))
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }))
    await waitFor(() =>
      expect(cli.posts).toContainEqual({ host: PRIMARY_HOST, route: 'projects/clear-icon', body: { projectId: 'p1' } }),
    )
    await waitFor(() => expect(fetchProjects).toHaveBeenCalledTimes(1))
  })

  it('Detect asks the daemon for the picture through projects/detect-icon', async () => {
    renderPrimary()
    fireEvent.click(screen.getByRole('button', { name: 'Detect' }))
    await waitFor(() =>
      expect(cli.posts).toContainEqual({ host: PRIMARY_HOST, route: 'projects/detect-icon', body: { projectId: 'p1' } }),
    )
  })

  it('a permission switch flips at once and writes its route', async () => {
    renderPrimary()
    fireEvent.click(sw('projects.agents-can-manage-agents'))
    expect(useProjectsStore.getState().projects[0].agentsCanManageAgents).toBe(1)
    await waitFor(() =>
      expect(cli.posts).toContainEqual({
        host: PRIMARY_HOST,
        route: 'agent-access/set',
        body: { workspace: '/ws/cortana', toggle: 'agents_manage', value: true },
      }),
    )
    fireEvent.click(sw('projects.mail-manage-enabled'))
    await waitFor(() =>
      expect(cli.posts).toContainEqual({
        host: PRIMARY_HOST,
        route: 'mail-manage',
        body: { project: '/ws/cortana', enable: 1 },
      }),
    )
    fireEvent.click(sw('projects.agents-can-manage-skin'))
    await waitFor(() =>
      expect(cli.posts).toContainEqual({
        host: PRIMARY_HOST,
        route: 'agents-manage-skin',
        body: { project: '/ws/cortana', enable: 1 },
      }),
    )
  })

  it('a rename from another client repaints the name (unsaved edits stay)', async () => {
    renderPrimary()
    const input = screen.getByLabelText('Agent Name') as HTMLInputElement
    await waitFor(() => expect(input.value).toBe('Cortana'))
    const hs = identity.handlers.filter((h) => h.host === PRIMARY_HOST)
    expect(hs.length).toBeGreaterThan(0)
    act(() => {
      for (const h of hs) {
        h.fn({ projectId: 'p1', workspacePath: '/ws/cortana', name: 'cortana', displayName: 'Cortana II', handle: 'cortana', fields: ['displayName'] })
      }
    })
    expect(input.value).toBe('Cortana II')
  })
})

describe('Agent tab: roles follow the route floors', () => {
  it('a Member sees Admin and Owner switches read-only, and can still change identity', async () => {
    role.value = 'member'
    renderPrimary()
    await waitFor(() => expect(sw('projects.agents-can-manage-agents').disabled).toBe(true))
    for (const id of ADMIN_SWITCHES) expect([id, sw(id).disabled]).toEqual([id, true])
    expect(sw('projects.agents-can-manage-skin').disabled).toBe(true)
    // workspace/set (db_agent_access) is a Member-floor route.
    expect(sw('projects.db-agent-create').disabled).toBe(false)
    // Operate compute nodes is hidden from a Member (as in Settings).
    expect(document.querySelector('[data-settings-id="projects.agents-can-operate-compute"]')).toBeNull()

    fireEvent.click(sw('projects.agents-can-manage-agents'))
    await act(async () => {})
    expect(cli.posts.filter((p) => p.route === 'agent-access/set')).toEqual([])

    await waitFor(() => expect((screen.getByLabelText('Agent Name') as HTMLInputElement).disabled).toBe(false))
  })

  it('an Admin may flip Admin switches but not the Owner-only Apps switch', async () => {
    role.value = 'admin'
    renderPrimary()
    await waitFor(() => expect(sw('projects.agents-can-manage-skin').disabled).toBe(true))
    for (const id of ADMIN_SWITCHES) expect([id, sw(id).disabled]).toEqual([id, false])
  })
})

describe('Agent tab: a room on another server', () => {
  function renderRemote(): void {
    const scope = fakeScope('b.k2.dev')
    const room = testRoom({
      tabs: {},
      isPrimary: false,
      scope,
      localCommands: false,
      projects: [project({ id: 'rp1', name: 'baden', path: '/home/k2/ai/baden', handle: 'baden', color: '#22c55e', agentsCanManageAgents: 1 })],
      activeProjectId: 'rp1',
    })
    renderInRoom(room, <AgentPanel />)
  }

  it('shows the agent read-only, read from that server, and writes nothing', async () => {
    renderRemote()
    expect(screen.getByTestId('agent-panel-readonly-note').textContent).toContain('b.k2.dev')
    await waitFor(() => expect(screen.getByTestId('agent-name-readonly').textContent).toBe('Baden Bot'))
    expect(screen.getByTestId('agent-handle-readonly').textContent).toBe('baden')
    expect(cli.gets.every((g) => g.host === 'b.k2.dev')).toBe(true)
    expect(screen.queryByLabelText('Agent Name')).toBeNull()
    expect(screen.queryByRole('button', { name: 'Upload' })).toBeNull()
    expect(screen.getByText('#22c55e')).toBeTruthy()

    const manage = sw('projects.agents-can-manage-agents')
    expect(manage.disabled).toBe(true)
    expect(manage.getAttribute('aria-checked')).toBe('true')
    for (const id of ADMIN_SWITCHES) expect([id, sw(id).disabled]).toEqual([id, true])
    fireEvent.click(manage)
    await act(async () => {})
    expect(cli.posts).toEqual([])
  })

  it('follows a rename on that server live', async () => {
    renderRemote()
    await waitFor(() => expect(screen.getByTestId('agent-name-readonly').textContent).toBe('Baden Bot'))
    act(() => {
      for (const h of identity.handlers.filter((x) => x.host === 'b.k2.dev')) {
        h.fn({ projectId: 'rp1', workspacePath: '/home/k2/ai/baden', name: 'baden', displayName: 'Baden', handle: 'baden2', fields: ['displayName', 'handle'] })
      }
    })
    expect(screen.getByTestId('agent-name-readonly').textContent).toBe('Baden')
    expect(screen.getByTestId('agent-handle-readonly').textContent).toBe('baden2')
  })
})
