// @vitest-environment jsdom
// Settings → Logs → Heartbeats: searching the list. Each word, in any case,
// must appear in a heartbeat's name, workspace, schedule, state words, where
// it runs, or its instructions file (WAKEUP.md). The instructions are read
// only once someone searches, from the window's server; a row found only by
// its instructions shows the line it was found in. Chips narrow to On /
// Paused / Needs attention with counts that follow the search.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const h = vi.hoisted(() => ({
  invokes: {} as Record<string, unknown>,
  routes: {} as Record<string, unknown>,
  files: {} as Record<string, string>,
  gets: [] as Array<{ scope: unknown; route: string; params: unknown }>,
  host: 'local',
  scope: { id: 'window-server', serverSupports: () => true },
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => h.invokes[cmd] ?? null),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: unknown, route: string, params: { path?: string }) => {
    h.gets.push({ scope, route, params })
    if (route === 'fs/read-file') {
      const path = params?.path ?? ''
      if (path in h.files) return { content: h.files[path] }
      throw new Error('not found')
    }
    return route in h.routes ? h.routes[route] : null
  }),
  daemonCliPost: vi.fn(async () => ({ success: true })),
  daemonCliPostQuery: vi.fn(async () => ({})),
}))
vi.mock('@/kessel/server-scope', () => ({ primaryScope: () => h.scope }))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => ({ activeHost: h.host }) },
}))
vi.mock('@/stores/tabs', () => ({ useTabsStore: {} }))
vi.mock('./HeartbeatsSection', () => ({ WakeupEditor: () => null }))
vi.mock('@/components/common/HeartbeatSessionPicker', () => ({
  HeartbeatSessionPicker: ({ ariaLabel }: { ariaLabel?: string }) => <button type="button" aria-label={ariaLabel}>Own session</button>,
  openHeartbeatTarget: vi.fn(),
}))
vi.mock('@/components/common/HeartbeatStatusLine', () => ({ HeartbeatStatusLine: () => null }))
vi.mock('@/hooks/useHeartbeatDueRefetch', () => ({ useHeartbeatDueRefetch: () => undefined }))

import { WakeSchedulerSection } from './WakeSchedulerSection'
import { plainText, snippetAround } from './heartbeat-search'

const row = (id: string, name: string, ws: string, frequency: string, specJson: string, over: Record<string, unknown> = {}) => ({
  id,
  projectId: `p-${ws}`,
  projectName: ws,
  projectPath: `/Users/alex/Projects/${ws}`,
  name,
  frequency,
  specJson,
  wakeupPath: `.k2/heartbeats/${name}/WAKEUP.md`,
  enabled: true,
  lastFired: null,
  useWorkspaceSession: false,
  lastSessionId: null,
  consecutiveFailures: 0,
  disabledReason: null,
  scheduleError: null,
  ...over,
})
const ROWS = [
  row('hb1', 'nightly-build', 'atlas', 'daily', '{"time":"02:00"}'),
  row('hb2', 'backup-check', 'atlas', 'daily', '{"time":"06:30"}'),
  row('hb3', 'weekly-report', 'borealis', 'weekly', '{"days":["mon"],"time":"09:00"}'),
  row('hb4', 'disk-sweep', 'borealis', 'weekly', '{"days":["sun"],"time":"23:00"}', { enabled: false, disabledReason: 'user' }),
  row('hb5', 'crm-cleanup', 'borealis', 'weekly', '{"days":["fri"],"time":"17:00"}', { enabled: false, disabledReason: 'failures', consecutiveFailures: 3 }),
]
const file = (ws: string, name: string): string => `/Users/alex/Projects/${ws}/.k2/heartbeats/${name}/WAKEUP.md`
const STATUS = {
  lastTickAt: '2026-11-10T09:00:00.000Z',
  staleSecs: 20,
  enabledCount: 3,
  transportInstalled: true,
  wakeMode: 'daemon',
  ticker: { intervalSecs: 60, lastTickAt: '2026-11-10T09:00:00.000Z', staleSecs: 20 },
}

beforeEach(() => {
  h.host = 'local'
  h.gets = []
  h.invokes = { k2_heartbeat_list_all: ROWS, k2_heartbeat_fires_list_all: [] }
  h.routes = { 'heartbeat/scheduler-status': STATUS }
  h.files = {
    [file('atlas', 'nightly-build')]: 'Run the build and post the results in the team chat.',
    [file('atlas', 'backup-check')]:
      '# Backup check\n\nEvery morning, check the **backup** `volume` is mounted and has room left.\n\n- Say so in [one line](https://example.com/notes).',
    [file('borealis', 'weekly-report')]: 'Summarise the week for the team.',
    [file('borealis', 'disk-sweep')]: 'Clear old logs.',
    [file('borealis', 'crm-cleanup')]: 'Merge duplicate contacts.',
  }
})
afterEach(() => cleanup())

async function ready(): Promise<HTMLInputElement> {
  await screen.findByTestId('heartbeat-row-hb5')
  return screen.getByRole('searchbox', { name: 'Search heartbeats' }) as HTMLInputElement
}
// In the page's order: by workspace, then name.
const shown = (): string[] =>
  Array.from(document.querySelectorAll('[data-testid^="heartbeat-row-"]')).map((e) => e.getAttribute('data-testid')!.slice('heartbeat-row-'.length))
const reads = (): string[] => h.gets.filter((g) => g.route === 'fs/read-file').map((g) => (g.params as { path: string }).path)
function type(field: HTMLInputElement, value: string): void {
  fireEvent.change(field, { target: { value } })
}

describe('Heartbeats search', () => {
  it('sits at the top of Your heartbeats, with the shared search field', async () => {
    render(<WakeSchedulerSection />)
    const field = await ready()
    expect(field.getAttribute('placeholder')).toBe('Search heartbeats')
    expect(field.closest('[data-k2-search-field]')).toBeTruthy()
    const list = screen.getByTestId('heartbeats-list')
    expect(list.contains(field)).toBe(true)
    expect(field.compareDocumentPosition(screen.getByTestId('heartbeat-row-hb1')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('matches the name, the workspace, the schedule and the state, every word anywhere, in any case', async () => {
    render(<WakeSchedulerSection />)
    const field = await ready()
    type(field, 'NIGHTLY')
    expect(shown()).toEqual(['hb1'])
    // The name's match is marked.
    expect(within(screen.getByTestId('heartbeat-row-hb1')).getByText('NIGHTLY'.toLowerCase(), { selector: 'mark' })).toBeTruthy()
    type(field, 'borealis')
    expect(shown()).toEqual(['hb5', 'hb4', 'hb3'])
    type(field, 'monday')
    expect(shown()).toEqual(['hb3'])
    type(field, '6:30')
    expect(shown()).toEqual(['hb2'])
    type(field, 'paused')
    expect(shown()).toEqual(['hb5', 'hb4'])
    type(field, 'failing borealis')
    expect(shown()).toEqual(['hb5'])
    type(field, 'own session atlas')
    expect(shown()).toEqual(['hb2', 'hb1'])
  })

  it('reads no instructions until the first search, then each file once, from the window’s server', async () => {
    render(<WakeSchedulerSection />)
    const field = await ready()
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(reads()).toEqual([])
    type(field, 'b')
    await waitFor(() => expect(reads().length).toBe(5))
    expect(new Set(reads())).toEqual(new Set(Object.keys(h.files)))
    type(field, 'ba')
    type(field, 'bac')
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(reads().length).toBe(5)
    for (const g of h.gets.filter((x) => x.route === 'fs/read-file')) expect(g.scope).toBe(h.scope)
  })

  it('finds a word only in the instructions and shows the line it is in, marked', async () => {
    render(<WakeSchedulerSection />)
    const field = await ready()
    type(field, 'volume')
    // Name matches don't wait for the files; this one joins when its file loads.
    await waitFor(() => expect(shown()).toEqual(['hb2']))
    const snippet = screen.getByTestId('heartbeat-snippet-hb2')
    // Plain text: no heading marks, emphasis, code ticks or link syntax.
    expect(snippet.textContent).toBe('Instructions: Backup check. Every morning, check the backup volume is mounted and has room left. Say so in one…')
    expect(within(snippet).getByText('volume', { selector: 'mark' })).toBeTruthy()
    // A row found by its name shows no snippet.
    type(field, 'backup')
    await waitFor(() => expect(shown()).toEqual(['hb2']))
    expect(screen.queryByTestId('heartbeat-snippet-hb2')).toBeNull()
  })

  it('the chips narrow the list, and their counts follow the search', async () => {
    render(<WakeSchedulerSection />)
    const field = await ready()
    expect(screen.getByRole('radio', { name: 'All (5)' })).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'On (3)' })).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'Paused (2)' })).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'Needs attention (1)' })).toBeTruthy()
    fireEvent.click(screen.getByRole('radio', { name: 'Paused (2)' }))
    expect(shown()).toEqual(['hb5', 'hb4'])
    fireEvent.click(screen.getByRole('radio', { name: 'Needs attention (1)' }))
    expect(shown()).toEqual(['hb5'])
    fireEvent.click(screen.getByRole('radio', { name: 'All (5)' }))
    type(field, 'borealis')
    expect(screen.getByRole('radio', { name: 'All (3)' })).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'On (1)' })).toBeTruthy()
    expect(screen.getByRole('radio', { name: 'Paused (2)' })).toBeTruthy()
    // More than one workspace: a picker for one of them.
    expect(screen.getByRole('button', { name: 'Workspace' })).toBeTruthy()
  })

  it('no match says so and offers Clear search; no heartbeats at all keeps the empty state', async () => {
    const { unmount } = render(<WakeSchedulerSection />)
    const field = await ready()
    type(field, 'zebra')
    await waitFor(() => expect(screen.getByText('No heartbeats match “zebra”.')).toBeTruthy())
    expect(shown()).toEqual([])
    const empty = screen.getByTestId('heartbeats-no-match')
    fireEvent.click(within(empty).getByRole('button', { name: 'Clear search' }))
    expect(field.value).toBe('')
    expect(shown().length).toBe(5)
    unmount()

    h.invokes.k2_heartbeat_list_all = []
    render(<WakeSchedulerSection />)
    await waitFor(() => expect(screen.getByText('No heartbeats yet')).toBeTruthy())
    expect(screen.queryByRole('searchbox', { name: 'Search heartbeats' })).toBeNull()
  })

  it('Esc in the field clears it; / focuses it', async () => {
    render(<WakeSchedulerSection />)
    const field = await ready()
    fireEvent.keyDown(document.body, { key: '/' })
    expect(document.activeElement).toBe(field)
    type(field, 'nightly')
    expect(shown()).toEqual(['hb1'])
    fireEvent.keyDown(field, { key: 'Escape' })
    expect(field.value).toBe('')
    expect(shown().length).toBe(5)
  })

  it('on another server, the list and the instructions both come from that server', async () => {
    h.host = 'remote'
    h.invokes = {}
    h.routes = { 'heartbeat/scheduler-status': STATUS, 'heartbeat/list-all': ROWS, 'heartbeat/fires-list-all': [] }
    render(<WakeSchedulerSection />)
    const field = await ready()
    type(field, 'volume')
    await waitFor(() => expect(shown()).toEqual(['hb2']))
    const list = h.gets.find((g) => g.route === 'heartbeat/list-all')
    expect(list?.scope).toBe(h.scope)
    const read = h.gets.filter((g) => g.route === 'fs/read-file')
    expect(read.length).toBe(5)
    for (const g of read) expect(g.scope).toBe(h.scope)
  })
})

describe('the instructions line reads as plain text', () => {
  it('drops heading marks, bullets and numbers, emphasis, code ticks and link syntax, keeping the words', () => {
    const md = [
      '## Weekly *report*',
      '',
      '1. Collect the **merged** PRs from `main`.',
      '2) Read __the notes__ and _flag_ risks.',
      '- See [the board](https://example.com/board) and ![chart](c.png).',
      '* Keep snake_case_names as they are.',
      '> Quote me.',
      '```',
      'echo hi',
      '```',
    ].join('\n')
    expect(plainText(md)).toBe(
      'Weekly report. Collect the merged PRs from main. Read the notes and flag risks. See the board and chart. Keep snake_case_names as they are. Quote me. echo hi',
    )
  })

  it('cuts a long text around the word, with … at each cut', () => {
    const text = plainText(`# Title\n\n${'word '.repeat(30)}needle ${'more '.repeat(30)}`)
    const s = snippetAround(text, 'needle')
    expect(s.startsWith('…')).toBe(true)
    expect(s.endsWith('…')).toBe(true)
    expect(s).toContain('needle')
  })
})
