// @vitest-environment jsdom
// Settings → Apps → This server, below the box setup (prd-agent-app-
// permissions-v1 rev 2, slice H): every App with its maintaining agent
// (name, address, picture/colour), state, memory budget and hostname; the
// agent's switches and cores read-only with a link to its card; every App's
// "Hand over…" sheet for the owner (0.46.5: the plan's lines, the options
// it asks for, then the card filed and approved with a fresh vault
// confirm); Undo and Finish inside the window; parked or orphaned Apps'
// Park / Uninstall; and the one-time review note with Review and Dismiss
// per agent. Reads GET /cli/app/list?box=1 and /cli/app/list on the
// window's server; Apps off hides it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  box: null as unknown,
  review: [] as unknown[],
  off: false,
  role: 'owner' as 'owner' | 'admin' | 'member' | null,
  /** Per route, refusals to answer before the call succeeds (in order). */
  refuse: {} as Record<string, Array<{ code: string; hint: string }>>,
  plan: null as unknown,
  handovers: [] as unknown[],
  confirmMethods: ['local-auth'] as string[],
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (route === 'projects/get-icon') return { found: false, dataUrl: null }
      if (route === 'app/handover/plan') return h.plan
      if (route === 'app/handover/status') return { ok: true, handovers: h.handovers }
      if (route !== 'app/list') throw new Error(`unexpected GET ${route}`)
      if (h.off) throw new Error(JSON.stringify({ error: { code: 'apps_disabled', hint: 'Apps (beta) are off' } }))
      if (params?.box === '1') return h.box
      return { ok: true, apps: [], accessReview: h.review }
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      const refusal = h.refuse[route]?.shift()
      if (refusal) throw new Error(JSON.stringify({ error: refusal }))
      if (route === 'agent-access/set' && body.toggle === 'app_review') {
        h.review = h.review.filter((r) => (r as { workspace: string }).workspace !== body.workspace)
      }
      if (route === 'app/handover' || route === 'app/unpark' || route === 'app/handover/undo') {
        return { ok: true, status: 'held', request: { id: 'w_card1' }, handover: { id: 'hov_1', state: 'held' } }
      }
      if (route === 'vault/confirm/start') return { methods: h.confirmMethods, expiresIn: 60 }
      if (route === 'vault/confirm/finish') return { token: 'tok-1', expiresIn: 60 }
      if (route === 'app/approve') return { ok: true, status: 'approved', request: { id: body.request } }
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

const PLAN = {
  ok: true,
  ready: true,
  planHash: 'abc',
  to: { handle: 'ops' },
  guests: { total: 2, byClass: { owner: 1, shared: 1 } },
  secrets: [{ secretId: 's1', name: 'STRIPE', case: 'move', collision: true }],
  database: { case: 'shared' },
  blockers: [],
  lines: [
    'Hand over shop from sales to ops: ops becomes the agent for shop; sales stops managing it the moment you approve.',
    'Keeps running. Nothing restarts.',
    'Guests: 2 guest(s) keep their access and stay signed in.',
  ],
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.box = BOX
  h.review = []
  h.off = false
  h.role = 'owner'
  h.refuse = {}
  h.plan = PLAN
  h.handovers = []
  h.confirmMethods = ['local-auth']
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
    // A healthy App: only "Hand over…" (no park or uninstall here).
    const actions = within(row).getByTestId('apps-row-shop-actions')
    expect(within(actions).getByRole('button', { name: 'Hand over…' })).toBeTruthy()
    expect(within(actions).queryByRole('button', { name: 'Park' })).toBeNull()
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
    expect(within(actions).queryByRole('button', { name: 'Park' })).toBeNull()
    fireEvent.click(within(actions).getByRole('button', { name: 'Unpark…' }))
    const sheet = await screen.findByTestId('apps-handover-old')
    fireEvent.change(within(sheet).getByLabelText('Agent for old'), { target: { value: 'ws-ops' } })
    await screen.findByTestId('apps-handover-old-plan')
    expect(h.gets.find((g) => g.route === 'app/handover/plan')?.params).toEqual({ app: 'old', to: 'ws-ops' })
    fireEvent.click(within(screen.getByTestId('apps-handover-old-plan')).getByRole('button', { name: 'Unpark' }))
    await waitFor(() => expect(h.posts.map((p) => p.route)).toEqual(['app/unpark', 'vault/confirm/start', 'vault/confirm/finish', 'app/approve']))
    expect(h.posts[0].body).toEqual({ app: 'old', to: 'ws-ops' })
    await screen.findByText('old is handed over. You can undo it for 72 hours.')

    fireEvent.click(within(screen.getByTestId('apps-row-old-actions')).getByRole('button', { name: 'Uninstall' }))
    expect(h.posts).toHaveLength(4)
    fireEvent.click(screen.getByRole('button', { name: 'Uninstall old (data stays)' }))
    await waitFor(() => expect(h.posts[4]).toEqual({ route: 'app/uninstall', body: { app: 'old' } }))
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
    fireEvent.click(within(screen.getByTestId('apps-row-lost-actions')).getByRole('button', { name: 'Park' }))
    await waitFor(() => expect(h.posts[0]).toEqual({ route: 'app/park', body: { app: 'lost' } }))
    cleanup()

    h.role = 'admin'
    render(<AppsBoxSection />)
    actions = await screen.findByTestId('apps-row-lost-actions')
    await waitFor(() => expect((within(actions).getByRole('button', { name: 'Hand over…' }) as HTMLButtonElement).disabled).toBe(true))
    expect(within(actions).getByRole('button', { name: 'Park' })).toBeTruthy()
    expect(within(actions).getByRole('button', { name: 'Uninstall' })).toBeTruthy()
    cleanup()

    h.role = 'member'
    render(<AppsBoxSection />)
    await screen.findByTestId('apps-row-lost')
    await act(async () => {})
    expect(screen.queryByTestId('apps-row-lost-actions')).toBeNull()
  })

  it('a handover the daemon refuses at filing (what changed since the plan) shows why and asks no confirm', async () => {
    const hint = 'what the handover would move changed since the card was asked for'
    h.refuse['app/unpark'] = [{ code: 'handover_stale', hint }]
    render(<AppsBoxSection />)
    fireEvent.click(within(await screen.findByTestId('apps-row-old-actions')).getByRole('button', { name: 'Unpark…' }))
    fireEvent.change(within(await screen.findByTestId('apps-handover-old')).getByLabelText('Agent for old'), { target: { value: 'ws-ops' } })
    fireEvent.click(within(await screen.findByTestId('apps-handover-old-plan')).getByRole('button', { name: 'Unpark' }))
    expect((await screen.findByTestId('apps-handover-old-error')).textContent).toContain(hint)
    expect(h.posts.map((p) => p.route)).toEqual(['app/unpark'])
  })

  it('Apps off hides the list', async () => {
    h.off = true
    const { container } = render(<AppsBoxSection />)
    await waitFor(() => expect(h.gets.length).toBeGreaterThan(0))
    await act(async () => {})
    expect(container.querySelector('[data-testid="apps-box"]')).toBeNull()
  })
})

describe('the Hand over… sheet (0.46.5)', () => {
  it('shows every line of the plan, asks for what the plan needs, then files the card and approves it with a confirm', async () => {
    render(<AppsBoxSection />)
    const row = await screen.findByTestId('apps-row-shop')
    fireEvent.click(within(row).getByRole('button', { name: 'Hand over…' }))
    const sheet = await screen.findByTestId('apps-handover-shop')
    // The current agent isn't offered.
    const options = Array.from((within(sheet).getByLabelText('Agent for shop') as HTMLSelectElement).options).map((o) => o.value)
    expect(options).toEqual(['', 'ws-ops'])
    fireEvent.change(within(sheet).getByLabelText('Agent for shop'), { target: { value: 'ws-ops' } })
    const plan = await screen.findByTestId('apps-handover-shop-plan')
    for (const line of PLAN.lines) expect(plan.textContent).toContain(line)
    // The plan's choices: the shared database, a secret name clash, shared guests.
    fireEvent.click(within(plan).getByLabelText(/Share the database/))
    await waitFor(() => expect(h.gets.filter((g) => g.route === 'app/handover/plan').at(-1)?.params).toEqual({ app: 'shop', to: 'ws-ops', db: 'share' }))
    expect(within(plan).getByLabelText(/Use the new agent’s own values/)).toBeTruthy()
    expect(within(plan).getByLabelText(/Take over the guests/)).toBeTruthy()
    fireEvent.click(within(screen.getByTestId('apps-handover-shop-plan')).getByRole('button', { name: 'Hand over' }))
    await waitFor(() => expect(h.posts.map((p) => p.route)).toEqual(['app/handover', 'vault/confirm/start', 'vault/confirm/finish', 'app/approve']))
    expect(h.posts[0].body).toEqual({ app: 'shop', to: 'ws-ops', db: 'share' })
    expect(h.posts[1].body).toEqual({ action: 'app-approve', secretId: 'w_card1' })
    expect(h.posts[3].body).toEqual({ request: 'w_card1', confirmToken: 'tok-1' })
    await screen.findByText('shop is handed over. You can undo it for 72 hours.')
  })

  it('a blocked plan says why and can’t be handed over; a password box asks for the password', async () => {
    h.plan = { ...PLAN, ready: false, blockers: [{ code: 'handover_db_busy', hint: 'its database is being upgraded', names: [] }] }
    render(<AppsBoxSection />)
    fireEvent.click(within(await screen.findByTestId('apps-row-shop')).getByRole('button', { name: 'Hand over…' }))
    fireEvent.change(within(await screen.findByTestId('apps-handover-shop')).getByLabelText('Agent for shop'), { target: { value: 'ws-ops' } })
    const plan = await screen.findByTestId('apps-handover-shop-plan')
    expect(within(plan).getByTestId('apps-handover-shop-blocker').textContent).toBe('its database is being upgraded')
    expect((within(plan).getByRole('button', { name: 'Hand over' }) as HTMLButtonElement).disabled).toBe(true)
    cleanup()

    h.plan = PLAN
    h.confirmMethods = ['vault-password']
    h.posts = []
    render(<AppsBoxSection />)
    fireEvent.click(within(await screen.findByTestId('apps-row-shop')).getByRole('button', { name: 'Hand over…' }))
    fireEvent.change(within(await screen.findByTestId('apps-handover-shop')).getByLabelText('Agent for shop'), { target: { value: 'ws-ops' } })
    fireEvent.click(within(await screen.findByTestId('apps-handover-shop-plan')).getByRole('button', { name: 'Hand over' }))
    const pw = await screen.findByLabelText('Password to confirm')
    fireEvent.change(pw, { target: { value: 'hunter2' } })
    fireEvent.click(screen.getByRole('button', { name: 'Confirm' }))
    await waitFor(() => expect(h.posts.map((p) => p.route)).toEqual(['app/handover', 'vault/confirm/start', 'vault/confirm/finish', 'app/approve']))
    expect(h.posts[2].body).toMatchObject({ method: 'vault-password', password: 'hunter2', secretId: 'w_card1' })
  })

  it('inside the window: Undo files the undo card and approves it; Finish closes the window', async () => {
    h.handovers = [{ id: 'hov_1', appId: 'shop', state: 'applied', fromHandle: 'ops', toHandle: 'sales', inWindow: true, windowUntil: 9e9 }]
    render(<AppsBoxSection />)
    const line = await screen.findByTestId('apps-row-shop-handover')
    expect(line.textContent).toContain('Handed over from ops to sales')
    fireEvent.click(within(line).getByRole('button', { name: 'Undo' }))
    await waitFor(() => expect(h.posts.map((p) => p.route)).toEqual(['app/handover/undo', 'vault/confirm/start', 'vault/confirm/finish', 'app/approve']))
    await screen.findByText('shop went back to ops.')
    fireEvent.click(within(screen.getByTestId('apps-row-shop-handover')).getByRole('button', { name: 'Finish' }))
    await waitFor(() => expect(h.posts.at(-1)).toEqual({ route: 'app/handover/finish', body: { app: 'shop' } }))
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
