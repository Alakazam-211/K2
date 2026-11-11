// @vitest-environment jsdom
// Settings → Logs → Heartbeats: a missing or non-list reply shows the error
// card instead of crashing; pausing confirms or says why it didn't take;
// Recent runs filters to problems and opens a run to say why.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  invokes: {} as Record<string, unknown>,
  routes: {} as Record<string, unknown>,
  posts: [] as Array<[string, unknown]>,
  enableFails: null as string | null,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => h.invokes[cmd] ?? null),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => (route in h.routes ? h.routes[route] : null)),
  daemonCliPost: vi.fn(async () => ({ success: true })),
  daemonCliPostQuery: vi.fn(async (_scope: unknown, route: string, params: unknown) => {
    h.posts.push([route, params])
    if (h.enableFails) throw new Error(h.enableFails)
    return {}
  }),
}))
vi.mock('@/kessel/server-scope', () => ({
  primaryScope: () => ({ serverSupports: () => true }),
}))
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))
vi.mock('@/stores/tabs', () => ({ useTabsStore: {} }))
vi.mock('./HeartbeatsSection', () => ({ WakeupEditor: () => null }))
vi.mock('@/components/common/HeartbeatSessionPicker', () => ({
  HeartbeatSessionPicker: ({ ariaLabel }: { ariaLabel?: string }) => <button type="button" aria-label={ariaLabel}>Own session</button>,
  openHeartbeatTarget: vi.fn(),
}))
vi.mock('@/components/common/HeartbeatStatusLine', () => ({ HeartbeatStatusLine: () => null }))
vi.mock('@/hooks/useHeartbeatDueRefetch', () => ({ useHeartbeatDueRefetch: () => undefined }))
vi.mock('@/lib/heartbeat-delivery', () => ({
  applyDeliveryTarget: (r: unknown) => r,
  deriveDeliveryTarget: () => ({ mode: 'own' }),
  setHeartbeatSession: vi.fn(async () => undefined),
}))

import { PAUSED_TEXT, WakeSchedulerSection, describeDecision, explainRun, listReply } from './WakeSchedulerSection'

const ROW = {
  id: 'hb1',
  projectId: 'p1',
  projectName: 'atlas',
  projectPath: '/Users/alex/Projects/atlas',
  name: 'nightly-build',
  frequency: 'daily',
  specJson: '{"time":"02:00"}',
  wakeupPath: '/Users/alex/Projects/atlas/.k2/heartbeats/nightly-build/WAKEUP.md',
  enabled: true,
  lastFired: null,
  useWorkspaceSession: false,
  lastSessionId: null,
  consecutiveFailures: 0,
  disabledReason: null,
  scheduleError: null,
}
const fire = (id: number, decision: string, reason: string | null = null) => ({
  id,
  projectId: 'p1',
  projectName: 'atlas',
  agentName: null,
  scheduleName: 'nightly-build',
  firedAt: '2026-11-10T09:00:00.000Z',
  mode: 'heartbeat',
  decision,
  reason,
  durationMs: null,
})
const STATUS = {
  lastTickAt: '2026-11-10T09:00:00.000Z',
  staleSecs: 20,
  enabledCount: 1,
  transportInstalled: true,
  wakeMode: 'daemon',
  ticker: { intervalSecs: 60, lastTickAt: '2026-11-10T09:00:00.000Z', staleSecs: 20 },
}

beforeEach(() => {
  h.invokes = {}
  h.routes = {}
  h.posts = []
  h.enableFails = null
})
afterEach(() => cleanup())

describe('listReply / describeDecision', () => {
  it('passes a list through and refuses anything else', () => {
    expect(listReply([1, 2])).toEqual([1, 2])
    for (const bad of [null, undefined, {}, { error: 'x' }, 'text']) expect(() => listReply(bad)).toThrow()
  })

  it('says each decision in plain words and marks the problems', () => {
    expect(describeDecision('fired')).toEqual({ text: 'Ran', tone: 'ok' })
    expect(describeDecision('wakeup_file_missing').tone).toBe('problem')
    expect(describeDecision('skipped_busy')).toEqual({ text: 'Skipped: the agent was busy', tone: 'skip' })
    expect(describeDecision('skipped_new_thing').text).toBe('Skipped: new thing')
  })

  it('explains a run’s reason in plain words with the next step, never the raw text', () => {
    expect(explainRun({ decision: 'error', reason: 'WAKEUP.md not found at .k2/heartbeats/nightly-build/WAKEUP.md' })).toEqual({
      text: 'Its instructions file is missing.',
      next: 'Edit its instructions to add them.',
      missingFile: true,
    })
    expect(explainRun({ decision: 'fired_catchup', reason: 'missed at 09:00 while the computer slept; fired 7h late' })?.text).toBe(
      'Ran 7 hours late because the computer was asleep.',
    )
    expect(explainRun({ decision: 'error', reason: 'the agent session exited before it answered' })?.text).toBe(
      'Failed: the agent’s chat closed before it answered.',
    )
    expect(explainRun({ decision: 'skipped_missed', reason: 'due while the computer slept; 14 hours late, so K2 skipped it' })?.next).toBe(
      'It runs again at its next time.',
    )
    expect(explainRun({ decision: 'fired', reason: null })).toBeNull()
  })
})

describe('WakeSchedulerSection', () => {
  it('null replies show the error cards with Try again instead of crashing', async () => {
    render(<WakeSchedulerSection />)
    await waitFor(() => expect(screen.getByText('Your heartbeats couldn’t load')).toBeTruthy())
    expect(screen.getByText('Recent runs couldn’t load')).toBeTruthy()
    expect(screen.getByText('The scheduler’s status couldn’t load')).toBeTruthy()
    // The page says what it couldn't read in its own words; a reply of the
    // wrong shape is never passed off as the server's reason.
    expect(screen.getByText(/couldn’t read the list of heartbeats on this server/)).toBeTruthy()
    expect(screen.queryByText(/The server said/)).toBeNull()
    expect(screen.queryByText(/reply wasn’t a list/)).toBeNull()
    expect(screen.queryByText('Something went wrong')).toBeNull()

    h.invokes.k2so_heartbeat_list_all = [ROW]
    fireEvent.click(screen.getAllByRole('button', { name: 'Try again' })[1])
    await waitFor(() => expect(screen.getByTestId('heartbeat-row-hb1')).toBeTruthy())
  })

  it('row buttons say what they do', async () => {
    h.invokes.k2so_heartbeat_list_all = [ROW]
    h.invokes.k2so_heartbeat_fires_list_all = []
    h.routes['heartbeat/scheduler-status'] = STATUS
    render(<WakeSchedulerSection />)
    const edit = await screen.findByRole('button', { name: 'Edit the instructions for nightly-build' })
    expect(edit.textContent).toBe('Edit instructions')
    expect(screen.queryByText('Edit wakeup')).toBeNull()
  })

  it('a row is two lines: the switch beside the text, the other controls under it', async () => {
    h.invokes.k2so_heartbeat_list_all = [ROW]
    h.invokes.k2so_heartbeat_fires_list_all = []
    h.routes['heartbeat/scheduler-status'] = STATUS
    render(<WakeSchedulerSection />)
    await screen.findByRole('button', { name: 'Edit the instructions for nightly-build' })
    const row = screen.getByTestId('heartbeat-row-hb1')
    const main = row.querySelector('[data-heartbeat-main]') as HTMLElement
    const actions = row.querySelector('[data-heartbeat-actions]') as HTMLElement
    expect(main.querySelector('[role="switch"]')).toBeTruthy()
    expect(main.querySelectorAll('button:not([role="switch"])').length).toBe(0)
    expect(actions.querySelector('[role="switch"]')).toBeNull()
    // The Runs in picker leads line 2.
    expect(actions.children.length).toBe(3)
    expect(actions.firstElementChild?.textContent).not.toMatch(/Open chat|Edit instructions/)
    expect(actions.textContent).toContain('Open chat')
    expect(actions.textContent).toContain('Edit instructions')
    // Line 2 comes after line 1 in the row.
    expect(main.compareDocumentPosition(actions) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('an empty list says how to add one', async () => {
    h.invokes.k2so_heartbeat_list_all = []
    h.invokes.k2so_heartbeat_fires_list_all = []
    h.routes['heartbeat/scheduler-status'] = STATUS
    render(<WakeSchedulerSection />)
    await waitFor(() => expect(screen.getByText('No heartbeats yet')).toBeTruthy())
    expect(screen.getByText('No runs yet')).toBeTruthy()
    expect(screen.getByText('Checking every minute')).toBeTruthy()
  })

  it('pausing confirms in the row; a failed pause puts the switch back and says why', async () => {
    h.invokes.k2so_heartbeat_list_all = [ROW]
    h.invokes.k2so_heartbeat_fires_list_all = []
    h.routes['heartbeat/scheduler-status'] = STATUS
    render(<WakeSchedulerSection />)
    const sw = await screen.findByRole('switch', { name: 'Run nightly-build in atlas' })
    fireEvent.click(sw)
    await waitFor(() => expect(screen.getByText('Saved')).toBeTruthy())
    // One wording for paused, and no stray state word.
    expect(screen.getByText(PAUSED_TEXT)).toBeTruthy()
    expect(screen.getByTestId('heartbeat-row-hb1').textContent).not.toMatch(/\bdisabled\b/)
    expect(h.posts[0]).toEqual(['heartbeat/enable', { project: ROW.projectPath, name: 'nightly-build', enabled: false }])
    expect(sw.getAttribute('aria-checked')).toBe('false')

    h.enableFails = 'heartbeat is locked'
    fireEvent.click(sw)
    await waitFor(() => expect(screen.getByText('Not saved')).toBeTruthy())
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toBe('Couldn’t resume nightly-build. Nothing changed; try again in a moment.The server said: heartbeat is locked.')
    expect(sw.getAttribute('aria-checked')).toBe('false')
  })

  it('Problems shows only failed runs, and opening one shows why', async () => {
    h.invokes.k2so_heartbeat_list_all = [ROW]
    h.invokes.k2so_heartbeat_fires_list_all = [fire(2, 'error', 'WAKEUP.md not found'), fire(1, 'fired')]
    h.routes['heartbeat/scheduler-status'] = STATUS
    const { container } = render(<WakeSchedulerSection />)
    await waitFor(() => expect(container.querySelectorAll('[data-fire-row]').length).toBe(2))
    fireEvent.click(screen.getByRole('radio', { name: 'Problems (1)' }))
    const rows = container.querySelectorAll('[data-fire-row]')
    expect(rows.length).toBe(1)
    expect(rows[0]?.textContent).toContain('Its instructions file is missing.')
    expect(rows[0]?.textContent).toContain('Edit its instructions to add them.')
    expect(rows[0]?.textContent).not.toContain('WAKEUP.md')
    const toggle = rows[0]?.querySelector('button') as HTMLButtonElement
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    // The scheduler's own words only in the opened run, labelled.
    const details = container.querySelector('[data-fire-details]') as HTMLElement
    expect(details.textContent).toContain('Log line')
    expect(details.textContent).toContain('error: WAKEUP.md not found')
    expect(screen.getAllByRole('button', { name: 'Edit instructions' }).length).toBeGreaterThan(0)
  })
})
