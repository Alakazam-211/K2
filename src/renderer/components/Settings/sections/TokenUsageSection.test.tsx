// @vitest-environment jsdom
//
// Usage v2 page (review-usage-page-v1 P1 + Phase 2): sections, the params
// it sends (no workspace for All, tz, since, group_by), the Owner/Admin
// gate by status, live refetch on token_usage_changed, inline errors,
// the activity log's order and cursors, cost labels, filters that reset on
// a server switch, the chat link, the focused room's server, and the
// two-column layout on a wide pane (breakdown beside the chart).

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { primaryScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import type { Room } from '@/stores/room'

type Params = Record<string, string> | undefined

const h = vi.hoisted(() => ({
  calls: [] as Array<{ scope: unknown; route: string; params: Record<string, string> | undefined }>,
  answer: null as null | ((route: string, params: Record<string, string> | undefined, scope: unknown) => unknown),
  tokenHandlers: new Set<() => void>(),
  openChat: vi.fn(),
  openSettings: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async (scope: unknown, route: string, params?: Record<string, string>) => {
    h.calls.push({ scope, route, params })
    if (!h.answer) throw new Error(`unexpected GET ${route}`)
    return h.answer(route, params, scope)
  },
  daemonCliPost: async (_scope: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  },
}))

vi.mock('@/stores/session-events', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/stores/session-events')>()
  return {
    ...actual,
    onTokenUsageChanged: (_scope: unknown, fn: () => void) => {
      h.tokenHandlers.add(fn)
      return () => h.tokenHandlers.delete(fn)
    },
    onSubscriptionUsageChanged: () => () => {},
    onLlmAccountsChanged: () => () => {},
  }
})

vi.mock('@/lib/usage-chat-link', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/usage-chat-link')>()
  return { ...actual, openUsageChat: async (row: unknown) => h.openChat(row) }
})

vi.mock('@/lib/open-settings-page', () => ({
  openSettingsPage: async (section: string) => h.openSettings(section),
}))

import { ROLE_GATE_TEXT, TokenUsageSection, USAGE_TWO_COLUMN_MIN_PX } from './TokenUsageSection'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { usePageViewStore } from '@/stores/page-view'
import { useWindowRoomStore } from '@/stores/window-room'
import { resetUsageLimitsForTests } from '@/stores/usage-limits'

const TZ = Intl.DateTimeFormat().resolvedOptions().timeZone

function totals(n: number, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    input_tokens: n,
    output_tokens: n * 2,
    cache_read_tokens: n * 10,
    cache_write_tokens: n,
    reasoning_tokens: 0,
    requests: n,
    turns: n,
    cost: { usd: n / 100, source: 'mixed', priced_requests: n - 1, provider_requests: 1, unpriced_requests: 1 },
    ...extra,
  }
}

function today(): string {
  const d = new Date()
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`
}

function report(params: Params): Record<string, unknown> {
  const harness = params?.harness
  const groupBy = params?.group_by
  const groups: Record<string, unknown[]> = {
    workspace: [
      { key: '/w/k2', label: '/w/k2', ...totals(30) },
      { key: '', label: 'Outside workspaces', ...totals(2) },
    ],
    model: [{ key: 'claude/claude-opus-5-5', label: 'claude-opus-5-5', ...totals(20) }],
    login: [{ key: 'acc_work', label: 'work', ...totals(25) }],
    harness: [
      { key: 'claude', label: 'claude', ...totals(25) },
      { key: 'codex', label: 'codex', ...totals(7) },
    ],
  }
  return {
    scope: 'machine',
    workspace: params?.workspace ?? null,
    tz: params?.tz ?? 'UTC',
    since_ms: null,
    until_ms: null,
    total: totals(harness === 'codex' ? 7 : harness === 'claude' ? 25 : 32),
    harnesses: harness ? [{ harness, ...totals(7) }] : [{ harness: 'claude', ...totals(25) }, { harness: 'codex', ...totals(7) }],
    models: [],
    workspaces: [],
    logins: [{ account_id: 'acc_work', tool: 'claude', label: 'work', ...totals(25) }],
    outside: totals(0),
    days: [{ day: today(), ...totals(harness === 'codex' ? 7 : 25) }],
    ...(groupBy ? { groups: groups[groupBy] } : {}),
  }
}

function turn(rowid: number, ms: number, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    rowid,
    cursor: `${ms}:${rowid}`,
    workspace_path: '/w/k2/sub',
    workspace: '/w/k2',
    outside: false,
    harness: 'claude',
    model: 'claude-opus-5-5',
    account_id: 'acc_work',
    session_id: `sess-${rowid}`,
    input_tokens: 10,
    output_tokens: 20,
    cache_read_tokens: 300,
    cache_write_tokens: 4,
    reasoning_tokens: 0,
    recorded_at: new Date(ms).toISOString(),
    recorded_ms: ms,
    cost_usd: 0.0012,
    cost_source: 'model',
    ...extra,
  }
}

const T = Date.now()
const LIMITS = {
  generatedAt: new Date(T).toISOString(),
  airgap: false,
  tools: [
    {
      tool: 'claude',
      display: 'Claude',
      supported: true,
      logins: [
        {
          accountId: 'acc_work',
          label: 'work',
          isDefault: true,
          isLive: true,
          pinned: [],
          state: 'ok',
          status: '',
          checkedAt: new Date(T).toISOString(),
          windows: [{ id: 'seven_day', label: 'Weekly', used: 0.62, usedPercent: 62, resetsAt: new Date(T + 3 * 3_600_000).toISOString() }],
        },
      ],
    },
  ],
}

let turnsPages: Array<(p: Params) => unknown | undefined> = []

function defaultAnswer(route: string, params: Params): unknown {
  if (route === 'usage/tokens') return report(params)
  if (route === 'usage/limits') return LIMITS
  if (route === 'usage/turns') {
    for (const page of turnsPages) {
      const got = page(params)
      if (got !== undefined) return got
    }
    return { rows: [turn(3, T - 1000), turn(2, T - 60_000), turn(1, T - 3_600_000)], next_cursor: `${T - 3_600_000}:1`, has_more: true }
  }
  throw new Error(`unexpected GET ${route}`)
}

function calls(route: string): Array<Record<string, string>> {
  return h.calls.filter((c) => c.route === route).map((c) => c.params ?? {})
}

function lastTokensCall(): Record<string, string> {
  const all = calls('usage/tokens').filter((p) => p.group_by !== undefined)
  if (all.length === 0) throw new Error('no usage/tokens report call')
  return all[all.length - 1]
}

const PROJECTS = [
  {
    id: 'p1',
    name: 'K2',
    path: '/w/k2',
    color: '',
    tabOrder: 0,
    lastOpenedAt: null,
    worktreeMode: 0,
    iconUrl: null,
    focusGroupId: null,
    pinned: 0,
    manuallyActive: 0,
    lastInteractionAt: null,
    workspaces: [{ id: 'ws1', projectId: 'p1', sectionId: null, type: 'main', branch: null, name: 'K2', tabOrder: 0, worktreePath: null, navVisible: 1 }],
  },
]

beforeEach(() => {
  cleanup()
  h.calls.length = 0
  h.tokenHandlers.clear()
  h.openChat.mockReset()
  h.openSettings.mockReset()
  h.answer = defaultAnswer
  turnsPages = []
  resetUsageLimitsForTests()
  useConnectHostStore.setState({ activeHost: 'local' })
  useProjectsStore.setState({ projects: PROJECTS as never, activeProjectId: 'p1', activeWorkspaceId: 'ws1' })
  usePageViewStore.setState({ page: 'agents' })
  useWindowRoomStore.setState({ focused: null })
})

afterEach(() => {
  cleanup()
  resetUsageLimitsForTests()
})

async function renderPage(): Promise<void> {
  render(<TokenUsageSection />)
  await screen.findByTestId('usage-tiles')
  await screen.findByTestId('usage-activity-rows')
}

describe('Usage page sections', () => {
  it('names the server and shows the limits strip, tiles with cost labels, chart, breakdown, footnote and activity', async () => {
    await renderPage()
    expect(screen.getByTestId('usage-title').textContent).toBe('Usage')
    expect(screen.getByTestId('usage-subtitle').textContent).toContain('Tokens your agents used on this computer')
    const strip = await screen.findByTestId('usage-limits-strip')
    expect(within(strip).getByTestId('usage-limits-strip-claude').textContent).toBe('ClaudeResets in 3h62%')
    expect(within(strip).getByRole('meter', { name: 'Claude limit used' }).getAttribute('aria-valuenow')).toBe('62')
    fireEvent.click(screen.getByTestId('usage-limits-strip-link'))
    await waitFor(() => expect(h.openSettings).toHaveBeenCalledWith('llm-tokens'))

    // Plain names for the token kinds; the exact count under the short one.
    expect(screen.getByTestId('usage-tile-input').textContent).toBe('Input3232 tokens')
    expect(screen.getByTestId('usage-tile-cache-read').textContent).toBe('Read from cache320320 tokens')
    expect(screen.getByTestId('usage-tile-cache-write').textContent).toBe('Written to cache3232 tokens')
    expect(screen.getByTestId('usage-tile-requests').textContent).toBe('Requests3232 calls')
    const cost = screen.getByTestId('usage-tile-cost')
    expect(cost.textContent).toContain('Estimated cost')
    expect(cost.textContent).toContain('$0.32')
    expect(screen.getByTestId('usage-tile-cost-unpriced').textContent).toBe('1 request unpriced')
    expect(cost.getAttribute('title')).toBe('An estimate at API list prices, not your bill.')

    await waitFor(() => expect(screen.getByTestId('usage-chart-legend').textContent).toBe('ClaudeCodex'))
    expect(screen.getByTestId('usage-breakdown-row-/w/k2').textContent).toContain('K2')
    expect(screen.getByTestId('usage-breakdown-row-outside').textContent).toContain('Outside workspaces')
    expect(screen.getByTestId('usage-footnote').textContent).toBe(
      'Input leaves out tokens read from or written to the cache. Output includes reasoning. A request is one call to a model (one prompt for Grok). Estimated cost uses API list prices; it isn’t your bill.',
    )
    const rows = within(screen.getByTestId('usage-activity-rows')).getAllByTestId(/^usage-activity-row-/)
    expect(rows.map((r) => r.getAttribute('data-testid'))).toEqual(['usage-activity-row-3', 'usage-activity-row-2', 'usage-activity-row-1'])
    expect(rows[0].textContent).toContain('K2')
    expect(rows[0].textContent).toContain('Claude · claude-opus-5-5 · work')
    expect(rows[0].textContent).toContain('Input 10 · Output 20 · Read from cache 300 · Written to cache 4')
    expect(screen.getByTestId('usage-activity-cost-3').textContent).toBe('≈ $0.0012')
    expect(screen.getByTestId('usage-activity-cost-3').getAttribute('title')).toBe(
      'An estimate at API list prices, not your bill. Source: list prices.',
    )
  })

  it('a total is a button: picking one charts it and marks it chosen', async () => {
    await renderPage()
    expect(screen.getByTestId('usage-tile-output').getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByRole('heading', { name: 'Output by day' })).toBeTruthy()
    fireEvent.click(screen.getByTestId('usage-tile-requests'))
    expect(screen.getByTestId('usage-tile-requests').getAttribute('aria-pressed')).toBe('true')
    expect(screen.getByTestId('usage-tile-output').getAttribute('aria-pressed')).toBe('false')
    expect(screen.getByRole('heading', { name: 'Requests by day' })).toBeTruthy()
  })

  it('a range with nothing in it shows one empty card that offers a wider range', async () => {
    h.answer = (route, params) => {
      if (route === 'usage/tokens') return { ...report(params), total: totals(0), days: [], groups: [] }
      if (route === 'usage/turns') return { rows: [], next_cursor: null, has_more: false }
      return defaultAnswer(route, params)
    }
    render(<TokenUsageSection />)
    const empty = await screen.findByTestId('usage-empty')
    expect(empty.textContent).toContain('No usage in the last 30 days')
    expect(screen.queryByTestId('usage-tiles')).toBeNull()
    expect(screen.queryByTestId('usage-chart')).toBeNull()
    fireEvent.click(screen.getByTestId('usage-empty-wider'))
    await waitFor(() => expect(screen.getByTestId('usage-empty').textContent).toContain('No usage in the last 90 days'))
    expect(screen.getByTestId('usage-range-90d').getAttribute('aria-checked')).toBe('true')
  })

  it('sends tz, since and group_by, and no workspace for All workspaces (B1, B4)', async () => {
    await renderPage()
    const first = lastTokensCall()
    const since = new Date()
    const start = new Date(since.getFullYear(), since.getMonth(), since.getDate() - 29).getTime()
    expect(first).toEqual({ since: String(start), tz: TZ, group_by: 'workspace' })
    // One chart series per harness: harness filter, no group_by, cost skipped.
    await waitFor(() => expect(calls('usage/tokens').filter((p) => p.harness)).toHaveLength(2))
    expect(calls('usage/tokens').filter((p) => p.harness).map((p) => p.harness).sort()).toEqual(['claude', 'codex'])
    expect(calls('usage/tokens').find((p) => p.harness === 'codex')).toEqual({ harness: 'codex', since: String(start), tz: TZ, cost: '0' })
    expect(calls('usage/turns')[0]).toEqual({ since: String(start), limit: '50' })

    fireEvent.click(screen.getByTestId('usage-range-all'))
    await waitFor(() => expect(lastTokensCall()).toEqual({ tz: TZ, group_by: 'workspace' }))
    fireEvent.click(screen.getByTestId('usage-tab-login'))
    await waitFor(() => expect(lastTokensCall().group_by).toBe('login'))
    expect(screen.getByTestId('usage-breakdown-row-acc_work').textContent).toContain('work')
  })

  it('clicking a breakdown row filters the whole page; the chip clears it; This workspace is a preset', async () => {
    await renderPage()
    fireEvent.click(within(screen.getByTestId('usage-breakdown-row-/w/k2')).getByRole('button', { name: /^Show only K2/ }))
    await waitFor(() => expect(lastTokensCall().workspace).toBe('/w/k2'))
    await waitFor(() => expect(calls('usage/turns').at(-1)?.workspace).toBe('/w/k2'))
    // The open workspace shows as "This workspace" in the bar, not as a chip.
    expect(screen.getByTestId('usage-this-workspace').getAttribute('aria-checked')).toBe('true')
    expect(screen.queryByTestId('usage-filter-workspace')).toBeNull()
    fireEvent.click(screen.getByTestId('usage-all-workspaces'))
    await waitFor(() => expect(lastTokensCall().workspace).toBeUndefined())
    expect(screen.getByTestId('usage-all-workspaces').getAttribute('aria-checked')).toBe('true')

    fireEvent.click(screen.getByTestId('usage-this-workspace'))
    await waitFor(() => expect(lastTokensCall().workspace).toBe('/w/k2'))

    fireEvent.click(screen.getByTestId('usage-tab-harness'))
    await screen.findByTestId('usage-breakdown-row-codex')
    fireEvent.click(within(screen.getByTestId('usage-breakdown-row-codex')).getByRole('button', { name: /^Show only Codex/ }))
    expect((await screen.findByTestId('usage-filter-harness')).textContent).toContain('Codex')
    await waitFor(() => expect(lastTokensCall()).toMatchObject({ workspace: '/w/k2', harness: 'codex', group_by: 'harness' }))
    // With a harness filter the report's own days are the one series: no per-harness GETs.
    const before = calls('usage/tokens').filter((p) => p.cost === '0').length
    fireEvent.click(screen.getByTestId('usage-clear-filters'))
    await waitFor(() => expect(lastTokensCall().harness).toBeUndefined())
    expect(lastTokensCall().workspace).toBeUndefined()
    await waitFor(() => expect(calls('usage/tokens').filter((p) => p.cost === '0').length).toBeGreaterThan(before))
  })

  it('resets the filters when the window switches servers (B24)', async () => {
    await renderPage()
    fireEvent.click(screen.getByTestId('usage-this-workspace'))
    await waitFor(() => expect(lastTokensCall().workspace).toBe('/w/k2'))
    act(() => {
      useConnectHostStore.setState({ activeHost: { id: 'r1', label: 'Rosson', hostname: 'rosson.k2.dev', port: 443, secure: true, token: 'tok' } as never })
    })
    await waitFor(() => expect(screen.getByTestId('usage-subtitle').textContent).toContain('used on rosson.k2.dev'))
    await waitFor(() => expect(lastTokensCall().workspace).toBeUndefined())
    expect(screen.queryByTestId('usage-filter-workspace')).toBeNull()
  })
})

describe('who may see it', () => {
  it("a Member's 403 shows the Owner/Admin note instead of the page, by status not text (B25, Q3)", async () => {
    h.answer = (route, params) => {
      if (route === 'usage/tokens') throw new DaemonHttpError('forbidden for some reason', 403, 'admin_only')
      if (route === 'usage/limits') throw new DaemonHttpError('nope', 403, 'admin_only')
      return defaultAnswer(route, params)
    }
    render(<TokenUsageSection />)
    expect((await screen.findByTestId('usage-role-gate')).textContent).toBe(ROLE_GATE_TEXT)
    expect(screen.queryByTestId('usage-tiles')).toBeNull()
    expect(screen.queryByTestId('usage-activity')).toBeNull()
    expect(screen.queryByTestId('usage-limits-strip')).toBeNull()
  })

  it('another failure says so in the Tokens section and keeps the rest of the page', async () => {
    h.answer = (route, params) => {
      if (route === 'usage/tokens') throw new DaemonHttpError('ledger busy', 500, null)
      return defaultAnswer(route, params)
    }
    render(<TokenUsageSection />)
    expect((await screen.findByTestId('usage-tokens-error')).textContent).toBe('ledger busy')
    expect(screen.getByText('Usage couldn’t load')).toBeTruthy()
    // Try again asks once more.
    const before = calls('usage/tokens').length
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }))
    await waitFor(() => expect(calls('usage/tokens').length).toBeGreaterThan(before))
    expect(screen.queryByTestId('usage-role-gate')).toBeNull()
    await screen.findByTestId('usage-activity-rows')
  })

  it('a failed chart series shows inline; the tiles and breakdown stay (B23)', async () => {
    h.answer = (route, params) => {
      if (route === 'usage/tokens' && params?.harness === 'codex') throw new Error('codex rows unreadable')
      return defaultAnswer(route, params)
    }
    await renderPage()
    expect((await screen.findByTestId('usage-chart-error')).textContent).toBe('codex rows unreadable. The totals and breakdown are still current.')
    expect(screen.getByText('The chart couldn’t load')).toBeTruthy()
    expect(screen.getByTestId('usage-tiles')).toBeTruthy()
    expect(screen.getByTestId('usage-breakdown')).toBeTruthy()
  })
})

describe('on a wide pane', () => {
  // jsdom has no layout: the test sets the page's measured width.
  let paneWidth = USAGE_TWO_COLUMN_MIN_PX
  const observers = new Set<() => void>()
  class FakeResizeObserver {
    private cb: () => void
    constructor(cb: () => void) {
      this.cb = cb
    }
    observe(): void {
      observers.add(this.cb)
    }
    unobserve(): void {}
    disconnect(): void {
      observers.delete(this.cb)
    }
  }
  const resizePane = (width: number): void => {
    paneWidth = width
    act(() => {
      for (const cb of observers) cb()
    })
  }
  const realClientWidth = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientWidth')

  beforeEach(() => {
    paneWidth = USAGE_TWO_COLUMN_MIN_PX
    Object.defineProperty(HTMLElement.prototype, 'clientWidth', {
      configurable: true,
      get(this: HTMLElement) {
        return this.dataset.usagePage !== undefined ? paneWidth : 0
      },
    })
    vi.stubGlobal('ResizeObserver', FakeResizeObserver)
  })

  afterEach(() => {
    cleanup()
    observers.clear()
    vi.unstubAllGlobals()
    if (realClientWidth) Object.defineProperty(HTMLElement.prototype, 'clientWidth', realClientWidth)
  })

  const page = (): HTMLElement => {
    const el = document.querySelector<HTMLElement>('[data-usage-page]')
    if (!el) throw new Error('no usage page')
    return el
  }

  it('puts the breakdown and plan limits beside the totals and chart, ranked by the picked total', async () => {
    h.answer = (route, params) => {
      // Codex wrote more output from fewer requests: the ranking follows the tile.
      if (route === 'usage/tokens' && params?.group_by === 'harness') {
        return { ...report(params), groups: [{ key: 'claude', label: 'claude', ...totals(25) }, { key: 'codex', label: 'codex', ...totals(7, { output_tokens: 150 }) }] }
      }
      return defaultAnswer(route, params)
    }
    await renderPage()
    expect(page().dataset.usageLayout).toBe('two-column')
    const columns = screen.getByTestId('usage-columns')
    const [left, right] = Array.from(columns.children) as HTMLElement[]
    expect(within(left).getByTestId('usage-tiles')).toBeTruthy()
    expect(within(left).getByTestId('usage-chart')).toBeTruthy()
    expect(within(right).getByTestId('usage-breakdown').dataset.breakdownLayout).toBe('list')
    expect(await within(right).findByTestId('usage-limits-strip')).toBeTruthy()
    // Activity spans both columns, under them.
    expect(columns.contains(screen.getByTestId('usage-activity'))).toBe(false)

    // One number per row: the picked total, its share and a bar.
    expect(screen.getByTestId('usage-breakdown-measure').textContent).toBe('Output')
    const k2 = screen.getByTestId('usage-breakdown-row-/w/k2')
    expect(within(k2).getByTestId('usage-breakdown-share').textContent).toBe('94%')
    expect(within(k2).getByRole('button', { name: 'Show only K2, Output 60, 94% of this range' })).toBeTruthy()
    expect(within(screen.getByTestId('usage-breakdown-row-outside')).getByTestId('usage-breakdown-share').textContent).toBe('6%')

    fireEvent.click(screen.getByTestId('usage-tab-harness'))
    await screen.findByTestId('usage-breakdown-row-codex')
    const order = (): string[] =>
      within(screen.getByTestId('usage-breakdown-rows'))
        .getAllByTestId(/^usage-breakdown-row-/)
        .map((r) => r.getAttribute('data-testid')?.replace('usage-breakdown-row-', '') ?? '')
    expect(order()).toEqual(['codex', 'claude'])
    fireEvent.click(screen.getByTestId('usage-tile-requests'))
    expect(screen.getByTestId('usage-breakdown-measure').textContent).toBe('Requests')
    expect(order()).toEqual(['claude', 'codex'])
    expect(within(screen.getByTestId('usage-breakdown-row-claude')).getByTestId('usage-breakdown-share').textContent).toBe('78%')
  })

  it('falls back to one column below the width, and follows the pane as it resizes', async () => {
    paneWidth = USAGE_TWO_COLUMN_MIN_PX - 1
    await renderPage()
    expect(page().dataset.usageLayout).toBe('one-column')
    expect(screen.queryByTestId('usage-columns')).toBeNull()
    expect(screen.getByTestId('usage-breakdown').dataset.breakdownLayout).toBe('table')
    // One column: plan limits lead, as before.
    const strip = await screen.findByTestId('usage-limits-strip')
    expect(strip.compareDocumentPosition(screen.getByTestId('usage-tokens')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    resizePane(USAGE_TWO_COLUMN_MIN_PX)
    expect(page().dataset.usageLayout).toBe('two-column')
    expect(screen.getByTestId('usage-breakdown').dataset.breakdownLayout).toBe('list')
    resizePane(USAGE_TWO_COLUMN_MIN_PX - 40)
    expect(page().dataset.usageLayout).toBe('one-column')
  })

  it('an empty range is one card across the page, not two columns', async () => {
    h.answer = (route, params) => {
      if (route === 'usage/tokens') return { ...report(params), total: totals(0), days: [], groups: [] }
      if (route === 'usage/turns') return { rows: [], next_cursor: null, has_more: false }
      return defaultAnswer(route, params)
    }
    render(<TokenUsageSection />)
    await screen.findByTestId('usage-empty')
    expect(page().dataset.usageLayout).toBe('one-column')
    expect(screen.queryByTestId('usage-columns')).toBeNull()
  })

  it('while the numbers load, the breakdown holds its place and says so', async () => {
    h.answer = (route, params) => (route === 'usage/tokens' ? new Promise(() => {}) : defaultAnswer(route, params))
    render(<TokenUsageSection />)
    const loading = await screen.findByTestId('usage-breakdown-loading')
    expect(loading.textContent).toContain('Loading the breakdown…')
    expect(screen.getByTestId('usage-columns').contains(loading)).toBe(true)
    expect(screen.getByTestId('usage-tiles').getAttribute('aria-busy')).toBe('true')
  })
})

describe('live and activity', () => {
  it('token_usage_changed refetches the totals and prepends newer rows, filling a gap (B14, B16)', async () => {
    await renderPage()
    const reportsBefore = calls('usage/tokens').filter((p) => p.group_by).length
    const head = `${T - 1000}:3`
    turnsPages = [
      (p) => (p?.after === head && !p?.before ? { rows: [turn(7, T + 4000), turn(6, T + 3000)], next_cursor: `${T + 3000}:6`, has_more: true } : undefined),
      (p) => (p?.after === head && p?.before === `${T + 3000}:6` ? { rows: [turn(5, T + 2000), turn(4, T + 1000)], next_cursor: null, has_more: false } : undefined),
    ]
    expect(h.tokenHandlers.size).toBe(2)
    act(() => {
      for (const fn of h.tokenHandlers) fn()
    })
    await waitFor(() => expect(screen.getByTestId('usage-activity-row-4')).toBeTruthy())
    const order = within(screen.getByTestId('usage-activity-rows'))
      .getAllByTestId(/^usage-activity-row-/)
      .map((r) => r.getAttribute('data-testid')?.replace('usage-activity-row-', ''))
    expect(order).toEqual(['7', '6', '5', '4', '3', '2', '1'])
    await waitFor(() => expect(calls('usage/tokens').filter((p) => p.group_by).length).toBeGreaterThan(reportsBefore))
  })

  it('Load older asks before the last cursor and says so when it fails', async () => {
    await renderPage()
    const older = `${T - 3_600_000}:1`
    turnsPages = [(p) => (p?.before === older ? { rows: [turn(0, T - 7_200_000, { session_id: '', cost_source: 'unpriced', cost_usd: null })], next_cursor: null, has_more: false } : undefined)]
    fireEvent.click(screen.getByTestId('usage-activity-more'))
    await screen.findByTestId('usage-activity-row-0')
    expect(screen.getByTestId('usage-activity-cost-0').textContent).toBe('No price')
    expect(screen.queryByTestId('usage-activity-open-0')).toBeNull()
    expect(screen.queryByTestId('usage-activity-more')).toBeNull()
    expect(screen.getByText('All 4 requests')).toBeTruthy()
  })

  it('a row with a session in a known workspace links to its chat', async () => {
    await renderPage()
    fireEvent.click(screen.getByTestId('usage-activity-open-3'))
    expect(h.openChat).toHaveBeenCalledWith(expect.objectContaining({ session_id: 'sess-3', workspace: '/w/k2', harness: 'claude' }))
  })
})

describe('a focused Home room on another server', () => {
  it("offers the room's server; choosing it sends every request there and drops the chat link", async () => {
    const roomScope = fakeScope('acme.k2.dev')
    const room = { key: 'acme', isPrimary: false, scope: roomScope } as unknown as Room
    usePageViewStore.setState({ page: 'home' })
    useWindowRoomStore.setState({ focused: room })
    await renderPage()
    expect(h.calls.every((c) => c.scope === primaryScope())).toBe(true)
    fireEvent.click(screen.getByTestId('usage-server-room'))
    await waitFor(() => expect(screen.getByTestId('usage-subtitle').textContent).toContain('used on acme.k2.dev'))
    await waitFor(() => expect(h.calls.some((c) => c.scope === roomScope && c.route === 'usage/tokens')).toBe(true))
    await waitFor(() => expect(h.calls.some((c) => c.scope === roomScope && c.route === 'usage/turns')).toBe(true))
    await screen.findByTestId('usage-activity-row-3')
    expect(screen.queryByTestId('usage-activity-open-3')).toBeNull()
    expect(screen.queryByTestId('usage-limits-strip-link')).toBeNull()
  })
})
