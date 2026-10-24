// @vitest-environment jsdom
// Settings → Apps → This server → Box usage (prd-app-resources-disk-guard-v1
// Slice B, 0.46.4): the disks as bars with the guard's three lines, the
// biggest users and agents, the box's CPU and memory over 24 h with each
// App's RAM budget, and clean: a dry run first, "Clean now" only for the
// owner and Admins. Everything comes from the daemon; Apps off hides it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: Record<string, unknown> | undefined }>,
  posts: [] as Array<{ route: string; body: Record<string, unknown> }>,
  usage: null as unknown,
  trends: null as unknown,
  off: false,
  noTrends: false,
  role: 'owner' as 'owner' | 'admin' | 'member' | null,
}))

vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(async (route: string, params?: Record<string, unknown>) => {
      h.gets.push({ route, params })
      if (h.off) throw new Error(JSON.stringify({ error: { code: 'apps_disabled', hint: 'Apps (beta) are off' } }))
      if (route === 'app/resources') return h.usage
      if (route === 'app/resources/trends') {
        if (h.noTrends) throw new Error(JSON.stringify({ error: { code: 'not_found', hint: 'no such App route' } }))
        return h.trends
      }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: primaryOnly(async (route: string, body: Record<string, unknown>) => {
      h.posts.push({ route, body })
      if (route === 'app/resources/clean') {
        const dry = body.dryRun === true
        return {
          ok: true,
          dryRun: dry,
          scope: 'box',
          items: [
            { kind: 'seed', id: 'aaaaaaaaaaaa', app: 'jobs', bytes: 2_600_000_000, removed: !dry },
            { kind: 'rotatedLog', id: 'apps/jobs/logs/d-1.log.1', app: 'jobs', bytes: 100_000_000, removed: !dry },
          ],
          totals: { bytes: 2_700_000_000, freedBytes: dry ? 0 : 2_700_000_000 },
          engine: { status: 'ok', note: null },
          never: 'Never touched: App data, databases, mail, uploads, workspace folders, transcripts, backups, the K2 Trash and the current base images.',
        }
      }
      return { ok: true, started: true }
    }),
    daemonCliPostQuery: vi.fn(),
  }
})
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => h.role) }))

import { AppsResourcesSection } from './AppsResourcesSection'
import { biggestRows, parseClean, parseUsage, sizeLabel, sparkPoints } from './app-resources-api'

const GIB = 1073741824

const USAGE = {
  ok: true,
  box: { host: 'nsi.k2.dev', cores: 8, memBytes: 32 * GIB, checkedAt: 1792345320 },
  guard: {
    mode: 'on',
    modeSource: 'setting',
    lines: { warnPct: 85, warnFreeBytes: 20 * GIB, refusePct: 95, reserveBytes: 10 * GIB, criticalPct: 98, criticalFreeBytes: 2 * GIB },
  },
  disks: [
    { id: '259:2', mount: '/', fsType: 'ext4', totalBytes: 480e9, usedBytes: 427e9, freeBytes: 53e9, usedPct: 89.0, level: 'warn', holds: ['k2Home', 'workspaces'], notMeasuredBytes: 260e9 },
  ],
  mem: { totalBytes: 32 * GIB, availableBytes: 10 * GIB, agentsBytes: null },
  cpu: { load5: 2.1 },
  consumers: [
    { kind: 'app', id: 'jobs', owner: { kind: 'agent', handle: 'agent-nsi', address: 'agent-nsi::nsi.k2.dev' }, diskBytes: 6.2e9, memBytes: 1.1 * GIB, budgetBytes: null },
    { kind: 'database', id: 'jobs_db', owner: { kind: 'agent', handle: 'agent-nsi' }, diskBytes: 58.2e9, memBytes: null },
    { kind: 'workspace', id: 'ws-1', owner: { kind: 'agent', handle: 'agent-nsi' }, diskBytes: 38.4e9, memBytes: null, folder: '/home/k2/AI Projects/nsi' },
    { kind: 'shared', id: 'mail', owner: { kind: 'server' }, diskBytes: 12.4e9, memBytes: null },
    { kind: 'service', id: 'plans-desk-api', owner: { kind: 'agent', handle: 'plans-desk' }, diskBytes: null, memBytes: 0.3 * GIB },
  ],
  byOwner: [
    { owner: { handle: 'agent-nsi', address: 'agent-nsi::nsi.k2.dev' }, diskBytes: 104e9 },
    { owner: { handle: 'plans-desk', address: 'plans-desk::nsi.k2.dev' }, diskBytes: 14e9 },
  ],
  k2WillClear: { seedsBytes: 2.6e9, seeds: 14, nextSweepAt: 1792400000 },
  scan: { workspacesAt: 1792338000, running: false },
  recent: [],
  suggestion: null,
}

const TRENDS = {
  ok: true,
  window: '24h',
  resSecs: 60,
  box: {
    cores: 8,
    memTotalBytes: 32 * GIB,
    points: [
      { at: 1, cpuCores: 1.5, memBytes: 20 * GIB },
      { at: 61, cpuCores: null, memBytes: 21 * GIB },
      { at: 121, cpuCores: 2.25, memBytes: 22 * GIB },
    ],
  },
  apps: [{ app: 'jobs', owner: { handle: 'agent-nsi' }, memBudgetBytes: 2 * GIB, summary: { memPeakBytes: 1.5 * GIB, cpuAvgCores: 0.4 }, points: [] }],
  services: [],
  budgets: { enforced: false },
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.usage = USAGE
  h.trends = TRENDS
  h.off = false
  h.noTrends = false
  h.role = 'owner'
})

afterEach(() => cleanup())

describe('app-resources-api', () => {
  it('parses the view; unknown stays null, never 0', () => {
    const u = parseUsage(USAGE)
    expect(u.mode).toBe('on')
    expect(u.disks[0]).toMatchObject({ mount: '/', level: 'warn', usedPct: 89 })
    expect(u.rows.find((r) => r.id === 'plans-desk-api')?.diskBytes).toBeNull()
    expect(u.k2WillClearBytes).toBe(2.6e9)
    expect(biggestRows(u.rows, 2).map((r) => r.id)).toEqual(['jobs_db', 'ws-1'])
    expect(parseUsage({}).disks).toEqual([])
    expect(parseUsage({ disks: [{ mount: '/', level: 'bogus' }] }).disks[0].level).toBe('ok')
  })

  it('sizes and sparklines', () => {
    expect(sizeLabel(null)).toBe('—')
    expect(sizeLabel(53e9)).toBe('53 GB')
    expect(sizeLabel(1.4e9)).toBe('1.4 GB')
    expect(sparkPoints([1, null, 2], 100, 10, 2)).toBe('0.0,5.0 100.0,0.0')
    expect(sparkPoints([null, 1], 100, 10, null)).toBe('')
    expect(parseClean({ items: [{ kind: 'seed', id: 'x', bytes: null }] }).items[0].bytes).toBeNull()
  })
})

describe('AppsResourcesSection', () => {
  it('draws the disks against the guard lines, the biggest users and the trends', async () => {
    render(<AppsResourcesSection />)
    const disk = await screen.findByTestId('box-disk-/')
    expect(within(disk).getByTestId('box-disk-level-/').textContent).toBe('Warn')
    expect(within(disk).getAllByTestId('box-disk-line')).toHaveLength(3)
    expect(within(disk).getByRole('meter').getAttribute('aria-valuenow')).toBe('89')
    expect(disk.textContent).toContain('53 GB free')
    expect(screen.getByTestId('box-guard').textContent).toContain('refuse new work at 95% or 10 GiB left')
    expect(screen.getByTestId('box-usage-row-database-jobs_db').textContent).toContain('58 GB')
    expect(screen.getByTestId('box-usage-row-shared-mail').textContent).toContain('this server')
    expect(screen.queryByTestId('box-usage-row-service-plans-desk-api')).toBeNull()
    expect(screen.getByTestId('box-usage-row-owner-agent-nsi').textContent).toContain('agent-nsi::nsi.k2.dev')
    expect(screen.getByText(/K2 clears 2.6 GB of its own old Seeds/)).not.toBeNull()
    await waitFor(() => expect(screen.getByTestId('box-trend-CPU').textContent).toContain('2.25 of 8 cores'))
    expect(screen.getByTestId('box-trend-RAM').querySelector('polyline')).not.toBeNull()
    expect(screen.getByTestId('box-budgets').textContent).toContain('jobs peak 1.5 GiB of its 2 GiB RAM budget')
    expect(h.gets.map((g) => g.route)).toEqual(['app/resources', 'app/resources/trends'])
    expect(h.gets[1].params).toEqual({ window: '24h' })
    expect(h.posts).toEqual([])
  })

  it('cleans with a dry run first; Clean now is the owner’s and Admins’ only', async () => {
    render(<AppsResourcesSection />)
    fireEvent.click(await screen.findByTestId('box-clean-plan'))
    const list = await screen.findByTestId('box-clean-plan-list')
    expect(h.posts).toEqual([{ route: 'app/resources/clean', body: { dryRun: true } }])
    expect(list.textContent).toContain('K2 can free about 2.7 GB')
    expect(list.textContent).toContain('Unused Seed (K2 copy)')
    expect(list.textContent).toContain('Never touched: App data')
    await waitFor(() => expect(screen.getByTestId('box-clean-now')).not.toBeNull())
    await act(async () => {
      fireEvent.click(screen.getByTestId('box-clean-now'))
    })
    await waitFor(() => expect(screen.getByTestId('box-usage-notice').textContent).toContain('Freed about 2.7 GB'))
    expect(h.posts[1]).toEqual({ route: 'app/resources/clean', body: { dryRun: false } })
    expect(screen.queryByTestId('box-clean-plan-list')).toBeNull()
  })

  it('a Member sees the dry run but no Clean now', async () => {
    h.role = 'member'
    render(<AppsResourcesSection />)
    fireEvent.click(await screen.findByTestId('box-clean-plan'))
    await screen.findByTestId('box-clean-plan-list')
    expect(screen.queryByTestId('box-clean-now')).toBeNull()
  })

  it('hides with Apps off and keeps the bars without trends', async () => {
    h.noTrends = true
    const { unmount } = render(<AppsResourcesSection />)
    await screen.findByTestId('box-disk-/')
    await waitFor(() => expect(screen.getByTestId('box-trend-CPU').textContent).toContain('not measured yet'))
    unmount()
    h.off = true
    const { container } = render(<AppsResourcesSection />)
    await waitFor(() => expect(h.gets.filter((g) => g.route === 'app/resources').length).toBe(2))
    await waitFor(() => expect(container.querySelector('[data-testid="apps-resources"]')).toBeNull())
  })
})
