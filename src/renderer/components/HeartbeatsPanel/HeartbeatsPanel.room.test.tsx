// @vitest-environment jsdom
// Heartbeat S4 (prd-heartbeat-firing-v1 HB28/HB29, T-S4b, T-S4d): the
// drawer in a Home room reads its rows through the room's ServerScope, so
// a remote room shows B's daemon truth; a `heartbeat_roster_changed`
// event refreshes the row; and a row whose `nextFireAt + 120 s` passes
// triggers exactly one refetch, not a poll. Fail loud — no skips.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, waitFor } from '@testing-library/react'

type Row = Record<string, unknown>
const daemon = vi.hoisted(() => ({
  /** Rows each fake server returns for `heartbeat/list`, by hostKey. */
  rows: {} as Record<string, Row[]>,
  /** Every request: [hostKey, route, params]. */
  calls: [] as Array<[string, string, unknown]>,
}))
const subs = vi.hoisted(() => ({
  calls: [] as Array<{ hostKey: string; path: string; handlers: Record<string, (e: unknown) => void> }>,
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    throw new Error(`a remote room must not run this computer's command ${cmd}`)
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    daemon.calls.push([scope.hostKey, route, params])
    if (route === 'heartbeat/list') {
      const rows = daemon.rows[scope.hostKey]
      if (!rows) throw new Error(`no fake daemon for ${scope.hostKey}`)
      return rows
    }
    if (route === 'heartbeat/list-archived') return []
    throw new Error(`unexpected route ${route}`)
  }),
  RecoveringError: class RecoveringError extends Error {},
}))
vi.mock('@/lib/terminal-daemon', () => ({
  terminalListRunning: vi.fn(async () => {
    throw new Error('a remote room must not list this computer’s PTYs')
  }),
}))
vi.mock('@/stores/session-events', () => ({
  subscribeToWorkspaceTabEvents: (
    scope: { hostKey: string },
    path: string,
    handlers: Record<string, (e: unknown) => void>,
  ) => {
    subs.calls.push({ hostKey: scope.hostKey, path, handlers })
    return () => undefined
  },
}))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: { getState: () => ({ openSettings: vi.fn() }) },
}))
vi.mock('@/lib/heartbeat-launch', () => ({ launchHeartbeat: vi.fn(async () => undefined) }))
vi.mock('@/components/common/HeartbeatSessionPicker', () => ({ openHeartbeatTarget: vi.fn() }))

import { HeartbeatsPanel } from './HeartbeatsPanel'
import { createHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { FIRING_GRACE_MS, REFETCH_SLACK_MS } from '@/lib/heartbeat-wait'

const MIN = 60_000
const iso = (t: number): string => new Date(t).toISOString()

function hbRow(over: Row): Row {
  return {
    id: 'hb-1',
    projectId: 'p-b',
    name: 'sew-build-drive',
    frequency: 'hourly',
    specJson: JSON.stringify({ frequency: 'hourly', every_seconds: 900 }),
    wakeupPath: '.k2/heartbeats/sew-build-drive/WAKEUP.md',
    enabled: true,
    lastFired: null,
    lastSessionId: null,
    archivedAt: null,
    createdAt: 0,
    activeTerminalId: null,
    ...over,
  }
}

const PROJECT = { id: 'p-b', path: '/home/k2/ai/sew', name: 'sew' } as unknown as ProjectWithWorkspaces

function roomFor(scope: ServerScope): ReturnType<typeof testRoom> {
  return testRoom({
    tabs: {},
    key: `room:${scope.hostKey}`,
    scope,
    isPrimary: false,
    localCommands: false,
    projects: [PROJECT],
    activeProjectId: 'p-b',
    heartbeats: createHeartbeatSessionsStore({ scope, localCommands: false }),
  })
}

function statusText(container: HTMLElement): string {
  const el = container.querySelector('[data-heartbeat-status]')
  if (!el) throw new Error('expected a heartbeat status line')
  const text = el.textContent
  if (text === null) throw new Error('status line has no text')
  return text
}

beforeEach(() => {
  daemon.rows = {}
  daemon.calls = []
  subs.calls = []
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

describe('T-S4b: a remote room shows B’s values', () => {
  it('reads the list through B’s scope and renders B’s nextFireAt', async () => {
    const scopeB = fakeScope('b.k2.dev')
    daemon.rows['b.k2.dev'] = [hbRow({ waitReason: 'scheduled', nextFireAt: iso(Date.now() + 12 * MIN + 30_000) })]
    // A (another server) would say something else; it must never be read.
    daemon.rows['a.k2.dev'] = [hbRow({ waitReason: 'no_ticks', waitSince: iso(Date.now() - 8 * 60 * MIN) })]

    const view = renderInRoom(roomFor(scopeB), <HeartbeatsPanel />)
    await waitFor(() => {
      expect(statusText(view.container)).toMatch(/^Next run: in 12m \d\ds$/)
    })
    expect(daemon.calls.length).toBeGreaterThan(0)
    expect(daemon.calls.every(([host]) => host === 'b.k2.dev')).toBe(true)
    expect(daemon.calls.find(([, route]) => route === 'heartbeat/list')?.[2]).toEqual({ project: '/home/k2/ai/sew' })
    // The live stream is B's too.
    expect(subs.calls.map((s) => [s.hostKey, s.path])).toEqual([['b.k2.dev', '/home/k2/ai/sew']])
  })

  it('a B older than heartbeat-next-fire renders schedule text only', async () => {
    const scopeOld: ServerScope = {
      ...fakeScope('old.k2.dev'),
      serverSupports: (f) => f !== 'heartbeat-next-fire',
    }
    daemon.rows['old.k2.dev'] = [hbRow({ lastFired: iso(Date.now() - 20 * MIN) })]
    const view = renderInRoom(roomFor(scopeOld), <HeartbeatsPanel />)
    await waitFor(() => {
      expect(view.getByText('Every 15m')).toBeTruthy()
    })
    expect(view.container.querySelector('[data-heartbeat-status]')).toBeNull()
    expect(view.queryByText(/Next run|now|firing/)).toBeNull()
  })
})

describe('an event refreshes the row', () => {
  it('heartbeat_roster_changed refetches B and the row shows the new reason', async () => {
    const scopeB = fakeScope('b.k2.dev')
    daemon.rows['b.k2.dev'] = [hbRow({ waitReason: 'scheduled', nextFireAt: iso(Date.now() + 5 * MIN) })]
    const view = renderInRoom(roomFor(scopeB), <HeartbeatsPanel />)
    await waitFor(() => {
      expect(statusText(view.container)).toMatch(/^Next run: in [45]m \d\ds$/)
    })
    expect(subs.calls.length).toBe(1)
    const listsBefore = daemon.calls.filter(([, r]) => r === 'heartbeat/list').length

    // The daemon's watchdog flagged it (HB22) and emitted the event (HB23).
    daemon.rows['b.k2.dev'] = [
      hbRow({
        waitReason: 'overdue',
        waitDetail: 'no_ticks: no scheduler tick since 2026-10-07T12:00:00Z',
        nextFireAt: iso(Date.now() - 6 * MIN - 10_000),
      }),
    ]
    const handler = subs.calls[0].handlers.onHeartbeatRosterChanged
    expect(typeof handler).toBe('function')
    await act(async () => {
      handler({ kind: 'heartbeat_roster_changed', projectId: 'p-b' })
    })
    await waitFor(() => {
      expect(statusText(view.container)).toBe('overdue 6m: scheduler not ticking')
    })
    expect(daemon.calls.filter(([, r]) => r === 'heartbeat/list').length).toBe(listsBefore + 1)
  })
})

describe('T-S4d: one refetch when nextFireAt + 120 s passes', () => {
  it('fires exactly once, then stays quiet while nothing changes', async () => {
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] })
    const now = Date.now()
    const scopeB = fakeScope('b.k2.dev')
    daemon.rows['b.k2.dev'] = [hbRow({ waitReason: 'scheduled', nextFireAt: iso(now + 10_000) })]
    const store = createHeartbeatSessionsStore({ scope: scopeB, localCommands: false })
    const lists = (): number => daemon.calls.filter(([, r]) => r === 'heartbeat/list').length

    await store.getState().refresh('/home/k2/ai/sew')
    expect(lists()).toBe(1)

    // Just before the deadline: nothing.
    await vi.advanceTimersByTimeAsync(10_000 + FIRING_GRACE_MS + REFETCH_SLACK_MS - 1000)
    expect(lists()).toBe(1)
    // The deadline passes: one refetch. The server still says the same
    // (stuck) thing, so the deadline is behind us and never re-arms.
    await vi.advanceTimersByTimeAsync(1000)
    expect(lists()).toBe(2)
    await vi.advanceTimersByTimeAsync(60 * MIN)
    expect(lists()).toBe(2)

    // A fresh nextFireAt (it fired) arms one new deadline.
    daemon.rows['b.k2.dev'] = [hbRow({ waitReason: 'scheduled', nextFireAt: iso(Date.now() + 15 * MIN) })]
    await store.getState().refresh('/home/k2/ai/sew')
    expect(lists()).toBe(3)
    await vi.advanceTimersByTimeAsync(15 * MIN + FIRING_GRACE_MS + REFETCH_SLACK_MS)
    expect(lists()).toBe(4)
    store.getState().clear()
  })

  it('a server without heartbeat-next-fire never arms the timer', async () => {
    vi.useFakeTimers({ toFake: ['Date', 'setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] })
    const scopeOld: ServerScope = {
      ...fakeScope('old.k2.dev'),
      serverSupports: (f) => f !== 'heartbeat-next-fire',
    }
    daemon.rows['old.k2.dev'] = [hbRow({ nextFireAt: iso(Date.now() + 10_000) })]
    const store = createHeartbeatSessionsStore({ scope: scopeOld, localCommands: false })
    await store.getState().refresh('/home/k2/ai/sew')
    await vi.advanceTimersByTimeAsync(60 * MIN)
    expect(daemon.calls.filter(([, r]) => r === 'heartbeat/list').length).toBe(1)
  })
})
