// @vitest-environment jsdom

// Settings → Compute, node sharing (prd-macos-tenant-compute-v1 §5, §10
// S1–S8): the Servers panel renders what the daemon reports (rows, the
// pending SAS, refusals, N of max, whether `team` is possible and why not),
// sends each gesture as it is (the daemon decides narrow vs widen and who
// may revoke or change the owner), offers "Make owner" only for eligible
// servers, shows a tenant its share with no admin controls, and renders
// the queue redacted as the daemon sends it. compute-api is mocked.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, waitFor, fireEvent, cleanup, within } from '@testing-library/react'

vi.mock('./compute-api', async (importOriginal) => {
  const orig = await importOriginal<typeof import('./compute-api')>()
  return {
    ...orig,
    fetchNodeServers: vi.fn(),
    addNodeServer: vi.fn(),
    setServerLimits: vi.fn(),
    pauseServer: vi.fn(),
    revokeServer: vi.fn(),
    changeNodeOwner: vi.fn(),
    fetchNodeQueue: vi.fn(),
  }
})

import * as api from './compute-api'
import type { ComputeNode, NodeServerRow, NodeServersOwner, ShareLimitsJson } from './compute-api'
import { NodeSharingPanel, changedLimits, parseForSecs } from './ComputeServersParts'

const m = vi.mocked(api)

const LIMITS: ShareLimitsJson = {
  weight: 1,
  top_class: 'interactive',
  gate: false,
  exclusive: false,
  max_running: 1,
  max_queued: 10,
  cpu_hours_day: 8,
  max_job_secs: 7200,
  max_cpus: 4,
  max_mem_gb: 8,
  disk_gb: 40,
  network: 'on',
}

const FP_B = 'b'.repeat(64)

function row(over: Partial<NodeServerRow>): NodeServerRow {
  return {
    slot: 1,
    label: 'z3thon.k2.dev',
    fp: FP_B,
    state: 'active',
    trust: 'own',
    role: 'tenant',
    limits: LIMITS,
    usage: { running: 1, queued: 2, cpu_secs_today: 5400, online: true },
    expiresAt: null,
    weight: 1,
    sas: null,
    refusal: null,
    ...over,
  }
}

function owned(over: Partial<NodeServersOwner> = {}): NodeServersOwner {
  return {
    node: 'mini-2',
    role: 'owner',
    shared: true,
    maxServers: 3,
    solo: false,
    servers: [
      row({ slot: 0, label: 'rosson.k2.dev', fp: 'a'.repeat(64), role: 'owner' }),
      row({}),
      row({ slot: 2, label: 'baden.k2.dev', trust: 'team', fp: 'c'.repeat(64) }),
      row({ slot: 3, label: 'new.k2.dev', state: 'pending', fp: '0123456789abcdef', sas: '482 913', usage: { online: false } }),
    ],
    requests: [],
    note: null,
    ownerActs: true,
    ...over,
  }
}

function node(over: Partial<ComputeNode> = {}): ComputeNode {
  return {
    id: 'n-on',
    name: 'mini-2',
    fingerprint: 'fp',
    os: 'linux',
    arch: 'x86_64',
    labels: {},
    state: 'active',
    enrolledVia: 'code',
    enrolledAt: 1,
    confirmedBy: 'owner',
    controllerPause: null,
    lastSeen: 1,
    lastRoute: null,
    protocol: 3,
    nodeVersion: '0.46.4',
    routes: [],
    online: true,
    liveRoute: 'relay',
    queued: 0,
    running: 0,
    offer: {
      control: 'active',
      available: true,
      unavailable: [],
      caps: { cpu_millis: 8000, mem_bytes: 16 * 2 ** 30, disk_budget_bytes: 100 * 2 ** 30, hard: true },
      free: { cpu_millis: 4000, mem_bytes: 8 * 2 ** 30, disk_free_bytes: 50 * 2 ** 30 },
      parallelMax: 2,
      tools: {},
      features: ['admin', 'servers', 'queue', 'servers-owner', 'tenant-wall'],
    },
    role: 'owner',
    share: {
      shared: true,
      role: 'owner',
      ownerLabel: null,
      servers: 3,
      state: 'active',
      expiresAt: null,
      expiresInSecs: null,
      limits: LIMITS,
      running: 0,
      queued: 0,
      cpuSecsToday: 0,
      line: null,
    },
    ...over,
  }
}

async function openServers(n: ComputeNode = node()): Promise<void> {
  render(<NodeSharingPanel node={n} />)
  fireEvent.click(screen.getByRole('button', { name: 'Servers' }))
  await screen.findByTestId('compute-servers-header')
}

beforeEach(() => {
  m.fetchNodeServers.mockResolvedValue(owned())
  m.fetchNodeQueue.mockResolvedValue({ node: 'mini-2', shared: true, from: 'node', entries: [] })
})

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('the Servers panel (S1)', () => {
  it('renders each line as the daemon reports it, with N of max and team', async () => {
    await openServers()
    expect(m.fetchNodeServers).toHaveBeenCalledWith('mini-2', false)
    expect(screen.getByTestId('compute-servers-header').textContent).toContain('3 of 3 servers')
    expect(screen.getByTestId('compute-servers-header').textContent).toContain('takes other people’s servers')
    const owner = screen.getByTestId('compute-server-row-0')
    expect(within(owner).getByText('owner')).toBeTruthy()
    expect(within(owner).queryByRole('button', { name: 'Revoke' })).toBeNull()
    expect(within(owner).queryByRole('button', { name: 'Pause' })).toBeNull()
    const b = screen.getByTestId('compute-server-row-1')
    expect(b.textContent).toContain('1 running · 2 queued · 1.5 CPU-h today · online')
    expect(b.textContent).toContain('no end date')
    expect(b.textContent).toContain('1 running · 10 queued · 8 CPU-h/day · 2h/job')
    const pend = screen.getByTestId('compute-server-row-3')
    expect(within(pend).getByTestId('compute-server-sas').textContent).toContain('482 913')
    expect(within(pend).getByTestId('compute-server-sas').textContent).toContain('read it to new.k2.dev')
  })

  it('says why a node takes no team servers (a Mac without per-tenant users)', async () => {
    const mac = node({ os: 'macos' })
    mac.offer = { ...mac.offer!, features: ['admin', 'servers', 'queue'] }
    await openServers(mac)
    expect(screen.getByTestId('compute-servers-header').textContent).toContain('a Mac without per-tenant users yet')
  })

  it('shows nothing on a node without the servers feature', () => {
    const old = node()
    old.offer = { ...old.offer!, features: ['admin'] }
    const { container } = render(<NodeSharingPanel node={old} />)
    expect(container.textContent).toBe('')
  })

  it('shows refusals and the open requests waiting for approval', async () => {
    m.fetchNodeServers.mockResolvedValue(
      owned({
        servers: [row({ slot: 0, label: 'rosson.k2.dev', role: 'owner' }), row({ state: 'refused', refusal: 'server_too_old' })],
        requests: [{ id: 'car_9', state: 'pending', summary: 'share mini-2 with the server at https://x.k2.dev as own' } as never],
      }),
    )
    await openServers()
    expect(screen.getByTestId('compute-server-row-1').textContent).toContain('server too old')
    expect(screen.getByTestId('compute-servers-waiting-car_9').textContent).toContain('Waiting for your approval: share mini-2')
  })
})

describe('limits (S2)', () => {
  it('sends only what changed; a narrowing says so, a widening waits for approval', async () => {
    m.setServerLimits.mockResolvedValueOnce({ ok: true, changed: true, class: 'operate', note: 'narrowed at once' })
    await openServers()
    const b = screen.getByTestId('compute-server-row-1')
    fireEvent.click(within(b).getByRole('button', { name: 'Edit limits' }))
    fireEvent.change(within(b).getByLabelText('Queued'), { target: { value: '5' } })
    fireEvent.click(within(b).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(m.setServerLimits).toHaveBeenCalledWith('mini-2', 'z3thon.k2.dev', { max_queued: 5 }, undefined))
    expect((await screen.findByTestId('compute-servers-notice')).textContent).toContain('narrowed at once')

    m.setServerLimits.mockResolvedValueOnce({ held: true, request: { id: 'car_2' } as never })
    fireEvent.click(within(screen.getByTestId('compute-server-row-1')).getByRole('button', { name: 'Edit limits' }))
    const form = screen.getByTestId('compute-limits-form')
    fireEvent.change(within(form).getByLabelText('Running at once'), { target: { value: '3' } })
    fireEvent.change(within(form).getByLabelText('End date from now'), { target: { value: 'none' } })
    fireEvent.click(within(form).getByRole('button', { name: 'Save' }))
    await waitFor(() => expect(m.setServerLimits).toHaveBeenLastCalledWith('mini-2', 'z3thon.k2.dev', { max_running: 3 }, 0))
    await waitFor(() => expect(screen.getByTestId('compute-servers-notice').textContent).toContain('Waiting for your approval'))
    expect(m.fetchNodeServers.mock.calls.length).toBeGreaterThanOrEqual(3)
  })

  it('refuses an empty change and a bad end date without calling the daemon', async () => {
    await openServers()
    const b = screen.getByTestId('compute-server-row-1')
    fireEvent.click(within(b).getByRole('button', { name: 'Edit limits' }))
    fireEvent.click(within(b).getByRole('button', { name: 'Save' }))
    expect(await within(b).findByText('Nothing changed')).toBeTruthy()
    fireEvent.change(within(b).getByLabelText('End date from now'), { target: { value: 'soon' } })
    fireEvent.click(within(b).getByRole('button', { name: 'Save' }))
    expect(await within(b).findByText('End date: e.g. 7d, 12h, or none')).toBeTruthy()
    expect(m.setServerLimits).not.toHaveBeenCalled()
  })
})

describe('add (S3)', () => {
  it('sends the add (held) and offers team only where the node walls tenants', async () => {
    m.addNodeServer.mockResolvedValue({ held: true, request: { id: 'car_3' } as never })
    const noWall = node()
    noWall.offer = { ...noWall.offer!, features: ['admin', 'servers', 'queue'] }
    await openServers(noWall)
    fireEvent.click(screen.getByRole('button', { name: 'Add server' }))
    const form = screen.getByTestId('compute-add-server')
    expect((within(form).getByLabelText('Another person’s server') as HTMLInputElement).disabled).toBe(true)
    expect(form.textContent).toContain('its tenant wall isn’t on')
    fireEvent.change(within(form).getByLabelText('New server address'), { target: { value: 'https://x.k2.dev' } })
    fireEvent.change(within(form).getByLabelText('Enroll code'), { target: { value: 'ABCDE-FGHJK.0123456789abcdef' } })
    fireEvent.change(within(form).getByLabelText('End date'), { target: { value: '30d' } })
    fireEvent.click(within(form).getByRole('button', { name: 'Add' }))
    await waitFor(() =>
      expect(m.addNodeServer).toHaveBeenCalledWith('mini-2', { url: 'https://x.k2.dev', code: 'ABCDE-FGHJK.0123456789abcdef', trust: 'own', forSecs: 30 * 86400 }),
    )
    expect((await screen.findByTestId('compute-servers-notice')).textContent).toContain('Waiting for your approval')
  })
})

describe('pause and resume (S4)', () => {
  it('sends pause for an active server and resume for a paused one', async () => {
    m.pauseServer.mockResolvedValue({ ok: true, note: 'its queued jobs wait' })
    m.fetchNodeServers.mockResolvedValue(owned({ servers: [row({}), row({ slot: 2, label: 'p.k2.dev', state: 'paused' })] }))
    await openServers()
    fireEvent.click(within(screen.getByTestId('compute-server-row-1')).getByRole('button', { name: 'Pause' }))
    await waitFor(() => expect(m.pauseServer).toHaveBeenCalledWith('mini-2', 'z3thon.k2.dev', 'pause'))
    fireEvent.click(within(screen.getByTestId('compute-server-row-2')).getByRole('button', { name: 'Resume' }))
    await waitFor(() => expect(m.pauseServer).toHaveBeenCalledWith('mini-2', 'p.k2.dev', 'resume'))
  })
})

describe('revoke (S5)', () => {
  it('asks first, with the two boxes, and sends what was ticked', async () => {
    m.revokeServer.mockResolvedValue({ ok: true, note: 'queued jobs cancelled' })
    await openServers()
    const b = screen.getByTestId('compute-server-row-1')
    fireEvent.click(within(b).getByRole('button', { name: 'Revoke' }))
    const dlg = screen.getByRole('dialog', { name: 'Revoke z3thon.k2.dev?' })
    fireEvent.click(within(dlg).getByLabelText('Stop running jobs now'))
    fireEvent.click(within(dlg).getByRole('button', { name: 'Revoke' }))
    await waitFor(() => expect(m.revokeServer).toHaveBeenCalledWith('mini-2', 'z3thon.k2.dev', { now: true, wipe: false }))
  })

  it('is the owner’s alone: disabled for anyone else', async () => {
    m.fetchNodeServers.mockResolvedValue(owned({ ownerActs: false }))
    await openServers()
    const b = screen.getByTestId('compute-server-row-1')
    expect((within(b).getByRole('button', { name: 'Revoke' }) as HTMLButtonElement).disabled).toBe(true)
    expect((within(b).getByRole('button', { name: 'Make owner' }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe('make owner (S6)', () => {
  it('is offered only for your own, active, enrolled, online servers on a node that can do it', async () => {
    await openServers()
    expect(within(screen.getByTestId('compute-server-row-1')).queryByRole('button', { name: 'Make owner' })).not.toBeNull()
    expect(within(screen.getByTestId('compute-server-row-2')).queryByRole('button', { name: 'Make owner' })).toBeNull()
    expect(within(screen.getByTestId('compute-server-row-3')).queryByRole('button', { name: 'Make owner' })).toBeNull()
    expect(within(screen.getByTestId('compute-server-row-0')).queryByRole('button', { name: 'Make owner' })).toBeNull()
    cleanup()
    const old = node()
    old.offer = { ...old.offer!, features: ['admin', 'servers', 'queue', 'tenant-wall'] }
    await openServers(old)
    expect(within(screen.getByTestId('compute-server-row-1')).queryByRole('button', { name: 'Make owner' })).toBeNull()
  })

  it('says what moves, and sends the choices', async () => {
    m.changeNodeOwner.mockResolvedValue({ held: true, request: { id: 'car_4' } as never })
    await openServers()
    fireEvent.click(within(screen.getByTestId('compute-server-row-1')).getByRole('button', { name: 'Make owner' }))
    const dlg = screen.getByRole('dialog', { name: 'Make z3thon.k2.dev the owner of mini-2?' })
    expect(dlg.textContent).toContain('drains')
    expect(dlg.textContent).toContain('stays as one of your servers')
    expect(within(dlg).getByTestId('compute-owner-key').textContent).toContain(FP_B)
    fireEvent.click(within(dlg).getByLabelText('Wipe the build caches'))
    fireEvent.click(within(dlg).getByRole('button', { name: 'Ask to change the owner' }))
    await waitFor(() => expect(m.changeNodeOwner).toHaveBeenCalledWith('mini-2', 'z3thon.k2.dev', { old: 'keep', caches: 'wipe', now: false }))
    expect((await screen.findByTestId('compute-servers-notice')).textContent).toContain('Waiting for your approval')
  })
})

describe('a tenant (S7)', () => {
  it('shows "Shared by …" and its share, with no admin controls', async () => {
    const t = node({
      role: 'tenant',
      share: {
        shared: true,
        role: 'tenant',
        ownerLabel: 'rosson.k2.dev',
        servers: 2,
        state: 'active',
        expiresAt: null,
        expiresInSecs: null,
        limits: LIMITS,
        running: 1,
        queued: 0,
        cpuSecsToday: 3600,
        line: null,
      },
    })
    t.offer = { ...t.offer!, features: ['servers', 'queue'] }
    render(<NodeSharingPanel node={t} />)
    const s = screen.getByTestId('compute-tenant-share')
    expect(s.textContent).toContain('Shared by rosson.k2.dev (2 servers)')
    expect(s.textContent).toContain('Your share: active · 1 running · 0 queued · 1.0 CPU-h today · no end date')
    expect(screen.queryByRole('button', { name: 'Servers' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Revoke' })).toBeNull()
    expect(m.fetchNodeServers).not.toHaveBeenCalled()
  })
})

describe('the queue (S8)', () => {
  it('renders the node’s queue with other servers redacted', async () => {
    m.fetchNodeQueue.mockResolvedValue({
      node: 'mini-2',
      shared: true,
      from: 'node',
      entries: [
        { pos: 1, mine: false, class: 'gate', state: 'running', cpuMillis: 8000, etaSecs: 360 },
        { pos: 2, mine: true, class: 'interactive', state: 'queued', short: 'abcd1234', argv: 'cargo test' },
      ],
    })
    render(<NodeSharingPanel node={node()} />)
    fireEvent.click(screen.getByRole('button', { name: 'Queue' }))
    const q = await screen.findByTestId('compute-queue-mini-2')
    await waitFor(() => expect(q.textContent).toContain('another server'))
    expect(q.textContent).toContain('abcd1234')
    expect(q.textContent).toContain('cargo test')
    expect(m.fetchNodeQueue).toHaveBeenCalledWith('mini-2')
  })
})

describe('helpers', () => {
  it('reads end dates and picks owner targets the daemon would take', () => {
    expect([parseForSecs('7d'), parseForSecs('12h'), parseForSecs('30m'), parseForSecs('none'), parseForSecs('x'), parseForSecs('')]).toEqual([
      7 * 86400, 12 * 3600, 1800, 0, null, null,
    ])
    expect(changedLimits(LIMITS, { max_queued: '10', max_running: '2', weight: 'x' })).toEqual({ max_running: 2 })
    const rows = owned().servers
    expect(api.ownerTargets(rows).map((r) => r.slot)).toEqual([1])
    expect(api.teamPossible({ os: 'linux', offer: undefined })).toEqual({ ok: false, why: 'its tenant wall isn’t on: your own servers only' })
  })
})
