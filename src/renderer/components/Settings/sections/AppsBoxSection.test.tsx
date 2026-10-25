// @vitest-environment jsdom
// Settings → Apps → This server, below the box setup (prd-agent-app-
// permissions-v1 rev 2, slice H): every App with its maintaining agent
// (name, address, picture/colour), state, memory budget and hostname; the
// agent's switches and cores read-only with a link to its card; parked or
// orphaned Apps with the owner's ways out (Unpark / Hand over / Park /
// Uninstall); and the one-time review note with Review and Dismiss per
// agent. Reads GET /cli/app/list?box=1 and /cli/app/list on the window's
// server; Apps off hides it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  box: null as unknown,
  review: [] as unknown[],
  off: false,
  role: 'owner' as 'owner' | 'admin' | 'member' | null,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (route === 'projects/get-icon') return { found: false, dataUrl: null }
      if (route !== 'app/list') throw new Error(`unexpected GET ${route}`)
      if (h.off) throw new Error(JSON.stringify({ error: { code: 'apps_disabled', hint: 'Apps (beta) are off' } }))
      if (params?.box === '1') return h.box
      return { ok: true, apps: [], accessReview: h.review }
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route === 'agent-access/set' && body.toggle === 'app_review') {
        h.review = h.review.filter((r) => (r as { workspace: string }).workspace !== body.workspace)
      }
      return { ok: true }
    }),
    daemonCliPostQuery: vi.fn(),
  }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => h.role) }))

import { useProjectsStore, type ProjectWithWorkspaces } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { AppsBoxSection } from './AppsBoxSection'
import { memoryLabel, parseBoxApps, postgresChip, stateLabel } from './app-access-api'

function project(id: string, name: string, over: Partial<ProjectWithWorkspaces> = {}): ProjectWithWorkspaces {
  return { id, name, path: `/ws/${name}`, color: '#22c55e', iconUrl: null, handle: name, workspaces: [], ...over } as ProjectWithWorkspaces
}

const BOX = {
  ok: true,
  box: true,
  apps: [
    {
      appId: 'shop',
      state: 'running',
      parked: false,
      maintainer: { agent: 'Sales', handle: 'sales', address: 'sales::rosson.k2.dev', kind: 'agent', workspaceId: 'ws-sales' },
      memory: { limitBytes: 512 * 1024 * 1024 },
      hostnames: ['shop.rosson.k2.dev'],
      switches: { buildAndRun: true, manageEndToEnd: false, cores: ['db', 'thread'] },
      database: { name: 'ws_sales', major: 16, source: 'default', sharedWith: [], requires: '>=16', available: [17] },
    },
    {
      appId: 'old',
      state: 'parked',
      parked: true,
      maintainer: { agent: null, handle: null, address: null, kind: 'parked' },
      memory: { limitBytes: 2 * 1024 ** 3 },
      hostnames: [],
      switches: null,
    },
  ],
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.box = BOX
  h.review = []
  h.off = false
  h.role = 'owner'
  useProjectsStore.setState({
    projects: [
      project('ws-sales', 'sales', { agentsCanManageSkin: 1, agentsCanManageAppsFull: 0, appCores: '["db","thread"]' }),
      project('ws-ops', 'ops'),
    ],
  })
})
afterEach(() => cleanup())

describe('parsing', () => {
  it('reads the box view, sorted, with plain-words state and memory', () => {
    const apps = parseBoxApps(BOX)
    expect(apps.map((a) => a.appId)).toEqual(['old', 'shop'])
    expect(apps[1].cores).toEqual(['db', 'thread'])
    expect(apps[1].maintainer.workspaceId).toBe('ws-sales')
    expect(apps[0].buildAndRun).toBeNull()
    expect(stateLabel(apps[0])).toBe('Parked (held by the owner)')
    expect(stateLabel({ state: 'stopped', parked: false, maintainer: { ...apps[1].maintainer, kind: 'removed' } })).toBe(
      'Unassigned (its workspace was removed)',
    )
    expect([memoryLabel(512 * 1024 ** 2), memoryLabel(2 * 1024 ** 3), memoryLabel(null)]).toEqual([
      '512 MiB',
      '2 GiB',
      'no memory limit',
    ])
  })

  it('reads the App database into the Postgres chip (0.46.4, PA17)', () => {
    const apps = parseBoxApps(BOX)
    expect(apps[1].database).toEqual({ name: 'ws_sales', major: 16, available: [17], sharedWith: [], unknown: null })
    expect(apps[0].database).toBeNull()
    expect(postgresChip(apps[1].database)).toBe('Postgres 16 · 17 available')
    expect(postgresChip({ name: 'ws_a', major: 17, available: [], sharedWith: ['b'], unknown: null })).toBe('Postgres 17')
    expect(postgresChip({ name: null, major: null, available: [], sharedWith: [], unknown: 'external' })).toBe(
      'Postgres: not a K2 database',
    )
    expect(postgresChip(null)).toBeNull()
  })
})

describe('the Apps rows', () => {
  it('shows each App with its agent, state, memory, hostname and the agent’s access read-only', async () => {
    render(<AppsBoxSection />)
    const row = await screen.findByTestId('apps-row-shop')
    expect(within(row).getByTestId('apps-row-shop-agent').textContent).toBe('Sales · sales::rosson.k2.dev')
    expect(row.textContent).toContain('running')
    expect(row.textContent).toContain('512 MiB · shop.rosson.k2.dev')
    expect(within(row).getByTestId('apps-row-shop-postgres').textContent).toBe('Postgres 16 · 17 available')
    expect(screen.queryByTestId('apps-row-old-postgres')).toBeNull()
    expect(within(row).getByTestId('apps-row-shop-access').textContent).toContain('Build and run Apps · uses Database, Thread')
    expect(within(row).queryByTestId('apps-row-shop-actions')).toBeNull()
    expect(h.gets.find((g) => g.route === 'app/list' && g.params?.box === '1')).toBeTruthy()
  })

  it('the agent’s switches follow the projects store live', async () => {
    render(<AppsBoxSection />)
    await screen.findByTestId('apps-row-shop')
    act(() => {
      useProjectsStore.setState({
        projects: [project('ws-sales', 'sales', { agentsCanManageSkin: 1, agentsCanManageAppsFull: 1, appCores: '["files"]' })],
      })
    })
    expect(screen.getByTestId('apps-row-shop-access').textContent).toContain(
      'Build and run, manage end to end · uses Files and rooms',
    )
  })

  it('Agent settings opens the maintaining agent’s card', async () => {
    const open = vi.fn()
    useSettingsStore.setState({ openSettings: open } as never)
    render(<AppsBoxSection />)
    const row = await screen.findByTestId('apps-row-shop')
    fireEvent.click(within(row).getByRole('button', { name: 'Agent settings' }))
    expect(open).toHaveBeenCalledWith('projects', 'ws-sales', 'agent')
  })

  it('a parked App: the owner unparks it to an agent or uninstalls it (with a second click)', async () => {
    render(<AppsBoxSection />)
    const row = await screen.findByTestId('apps-row-old')
    expect(row.textContent).toContain('Parked (held by the owner)')
    expect(row.textContent).toContain('No agent maintains it')
    const actions = await within(row).findByTestId('apps-row-old-actions')
    const unpark = within(actions).getByRole('button', { name: 'Unpark' }) as HTMLButtonElement
    expect(unpark.disabled).toBe(true)
    expect(within(actions).queryByRole('button', { name: 'Park' })).toBeNull()
    fireEvent.change(within(actions).getByLabelText('Agent for old'), { target: { value: 'ws-ops' } })
    fireEvent.click(unpark)
    await waitFor(() => expect(h.posts).toEqual([{ route: 'app/unpark', body: { app: 'old', to: 'ws-ops' } }]))
    await screen.findByText('old is unparked: it is stopped until its agent starts it.')

    fireEvent.click(within(screen.getByTestId('apps-row-old-actions')).getByRole('button', { name: 'Uninstall' }))
    expect(h.posts).toHaveLength(1)
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall old (data stays)' }))
    await waitFor(() => expect(h.posts[1]).toEqual({ route: 'app/uninstall', body: { app: 'old' } }))
  })

  it('an orphan (its workspace removed): hand over and park; an admin gets park and uninstall only; a member nothing', async () => {
    h.box = {
      apps: [
        {
          appId: 'lost',
          state: 'stopped',
          parked: false,
          maintainer: { agent: null, handle: null, address: null, kind: 'removed' },
          memory: { limitBytes: null },
          hostnames: [],
          switches: null,
        },
      ],
    }
    render(<AppsBoxSection />)
    let actions = await screen.findByTestId('apps-row-lost-actions')
    expect(screen.getByTestId('apps-row-lost').textContent).toContain('Unassigned (its workspace was removed)')
    fireEvent.change(within(actions).getByLabelText('Agent for lost'), { target: { value: 'ws-sales' } })
    fireEvent.click(within(actions).getByRole('button', { name: 'Hand over' }))
    await waitFor(() => expect(h.posts).toEqual([{ route: 'app/handover', body: { app: 'lost', to: 'ws-sales' } }]))
    fireEvent.click(within(screen.getByTestId('apps-row-lost-actions')).getByRole('button', { name: 'Park' }))
    await waitFor(() => expect(h.posts[1]).toEqual({ route: 'app/park', body: { app: 'lost' } }))
    cleanup()

    h.role = 'admin'
    render(<AppsBoxSection />)
    actions = await screen.findByTestId('apps-row-lost-actions')
    await waitFor(() => expect(within(actions).queryByRole('button', { name: 'Hand over' })).toBeNull())
    expect(within(actions).getByRole('button', { name: 'Park' })).toBeTruthy()
    expect(within(actions).getByRole('button', { name: 'Uninstall' })).toBeTruthy()
    cleanup()

    h.role = 'member'
    render(<AppsBoxSection />)
    await screen.findByTestId('apps-row-lost')
    await act(async () => {})
    expect(screen.queryByTestId('apps-row-lost-actions')).toBeNull()
  })

  it('Apps off hides the list', async () => {
    h.off = true
    const { container } = render(<AppsBoxSection />)
    await waitFor(() => expect(h.gets.length).toBeGreaterThan(0))
    await act(async () => {})
    expect(container.querySelector('[data-testid="apps-box"]')).toBeNull()
  })
})

describe('the review note (Q4)', () => {
  it('lists the agents that kept Build and run Apps, Review opens the card, Dismiss drops one', async () => {
    h.review = [
      { workspace: 'ws-sales', handle: 'sales', level: 'build', notedAt: 1 },
      { workspace: 'ws-ops', handle: 'ops', level: 'build', notedAt: 1 },
    ]
    const open = vi.fn()
    useSettingsStore.setState({ openSettings: open } as never)
    render(<AppsBoxSection />)
    const note = await screen.findByTestId('apps-review-note')
    expect(note.textContent).toContain('These agents could manage Apps before; they now have Build and run Apps on.')
    fireEvent.click(within(screen.getByTestId('apps-review-ws-sales')).getByRole('button', { name: 'Review' }))
    expect(open).toHaveBeenCalledWith('projects', 'ws-sales', 'agent')
    fireEvent.click(within(screen.getByTestId('apps-review-ws-ops')).getByRole('button', { name: 'Dismiss' }))
    await waitFor(() =>
      expect(h.posts).toEqual([{ route: 'agent-access/set', body: { workspace: 'ws-ops', toggle: 'app_review', value: false } }]),
    )
    await waitFor(() => expect(screen.queryByTestId('apps-review-ws-ops')).toBeNull())
    expect(screen.getByTestId('apps-review-ws-sales')).toBeTruthy()
  })

  it('is not shown to a member', async () => {
    h.role = 'member'
    h.review = [{ workspace: 'ws-sales', handle: 'sales', level: 'build', notedAt: 1 }]
    render(<AppsBoxSection />)
    await screen.findByTestId('apps-row-shop')
    await act(async () => {})
    expect(screen.queryByTestId('apps-review-note')).toBeNull()
  })
})
