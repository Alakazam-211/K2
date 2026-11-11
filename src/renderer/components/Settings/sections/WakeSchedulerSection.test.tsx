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

import { WakeSchedulerSection, describeDecision, listReply } from './WakeSchedulerSection'

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
})

describe('WakeSchedulerSection', () => {
  it('null replies show the error cards with Try again instead of crashing', async () => {
    render(<WakeSchedulerSection />)
    await waitFor(() => expect(screen.getByText('Your heartbeats couldn’t load')).toBeTruthy())
    expect(screen.getByText('Recent runs couldn’t load')).toBeTruthy()
    expect(screen.getByText('K2 couldn’t read the scheduler')).toBeTruthy()
    expect(screen.getAllByText('The server said: the reply wasn’t a list.').length).toBe(2)
    expect(screen.queryByText('Something went wrong')).toBeNull()

    h.invokes.k2so_heartbeat_list_all = [ROW]
    fireEvent.click(screen.getAllByRole('button', { name: 'Try again' })[1])
    await waitFor(() => expect(screen.getByTestId('heartbeat-row-hb1')).toBeTruthy())
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
    await waitFor(() => expect(screen.getByText('Paused. It won’t run until you turn it back on.')).toBeTruthy())
    expect(h.posts[0]).toEqual(['heartbeat/enable', { project: ROW.projectPath, name: 'nightly-build', enabled: false }])
    expect(sw.getAttribute('aria-checked')).toBe('false')

    h.enableFails = 'heartbeat is locked'
    fireEvent.click(sw)
    await waitFor(() => expect(screen.getByText('Couldn’t resume it: heartbeat is locked. Nothing changed; try again.')).toBeTruthy())
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
    expect(rows[0]?.textContent).toContain('Failed')
    const toggle = rows[0]?.querySelector('button') as HTMLButtonElement
    fireEvent.click(toggle)
    expect(toggle.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByText('WAKEUP.md not found')).toBeTruthy()
  })
})
