// @vitest-environment jsdom
// Heartbeat S4 (prd-heartbeat-firing-v1 HB26–HB30, T6): the drawer row
// renders the daemon's `nextFireAt` + `waitReason`. It does no schedule
// math, never shows a bare "now" or "0m 0s", and a server without
// `heartbeat-next-fire` shows the schedule text only. S5's wait copy
// (empty WAKEUP.md, invalid schedule) is kept. Fail loud — no skips, no
// daemon.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => {
    throw new Error('the row must not call the daemon while rendering')
  }),
}))
vi.mock('@/lib/heartbeat-launch', () => ({ launchHeartbeat: vi.fn(async () => undefined) }))
vi.mock('@/components/common/HeartbeatSessionPicker', () => ({ openHeartbeatTarget: vi.fn() }))
vi.mock('@/stores/heartbeat-sessions', () => ({
  useHeartbeatSessionsStore: { getState: () => ({ refresh: vi.fn() }) },
}))

import { HeartbeatEntryRow } from './HeartbeatEntry'
import type { HeartbeatEntry, HeartbeatRow } from '@/stores/heartbeat-sessions'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import type { ServerScope } from '@/kessel/server-scope'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '../../../..')
const REASONS: string[] = JSON.parse(
  readFileSync(join(ROOT, 'crates/k2-core/src/heartbeats/fixtures/wait-reasons.json'), 'utf8'),
)

// Fixed local clock: 2026-10-07 14:00:00.
const NOW = new Date(2026, 9, 7, 14, 0, 0).getTime()
const iso = (t: number): string => new Date(t).toISOString()
const MIN = 60_000
const HOUR = 3_600_000

/** B: a server on a version with `heartbeat-next-fire`. */
const scopeNew: ServerScope = fakeScope('b.k2.dev')
/** C: a server older than `heartbeat-next-fire`. */
const scopeOld: ServerScope = {
  ...fakeScope('old.k2.dev'),
  serverSupports: (f) => f !== 'heartbeat-next-fire',
}

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'clearInterval', 'setTimeout', 'clearTimeout'] })
  vi.setSystemTime(NOW)
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

function entry(row: Partial<HeartbeatRow>): HeartbeatEntry {
  return {
    row: {
      id: 'id',
      projectId: 'p',
      name: 'hb',
      frequency: 'hourly',
      specJson: JSON.stringify({ frequency: 'hourly', every_seconds: 900 }),
      wakeupPath: '.k2/heartbeats/hb/WAKEUP.md',
      enabled: true,
      lastFired: null,
      lastSessionId: null,
      archivedAt: null,
      createdAt: Math.floor((NOW - 10_000) / 1000),
      ...row,
    },
    state: 'scheduled',
    liveTerminalId: null,
  }
}

function renderRow(e: HeartbeatEntry, scope: ServerScope = scopeNew): ReturnType<typeof renderInRoom> {
  return renderInRoom(
    testRoom({ tabs: {}, scope, isPrimary: false, localCommands: false }),
    <HeartbeatEntryRow entry={e} projectPath="/ws/proj" />,
  )
}

/** The status line's text (fails loudly when there is none). */
function statusText(view: ReturnType<typeof renderInRoom>): string {
  const el = view.container.querySelector('[data-heartbeat-status]')
  if (!el) throw new Error('expected a heartbeat status line')
  const text = el.textContent
  if (text === null) throw new Error('status line has no text')
  return text
}

describe('T6: the row renders the daemon next fire, never "now"', () => {
  it('a heartbeat that has never fired shows the daemon nextFireAt as a countdown', () => {
    // createdAt = now − 10 s, every 900 s → the daemon stores createdAt + 900.
    const view = renderRow(entry({ waitReason: 'scheduled', nextFireAt: iso(NOW - 10_000 + 900_000) }))
    expect(statusText(view)).toBe('Next run: in 14m 50s')
    expect(view.queryByText(/^now$/)).toBeNull()
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(statusText(view)).toBe('Next run: in 14m 49s')
  })

  it('reaching nextFireAt reads "firing…", not "now" or "0m 0s"', () => {
    const view = renderRow(entry({ waitReason: 'scheduled', nextFireAt: iso(NOW + 2000) }))
    expect(statusText(view)).toBe('Next run: in 2s')
    act(() => {
      vi.advanceTimersByTime(2000)
    })
    expect(statusText(view)).toBe('firing…')
    act(() => {
      vi.advanceTimersByTime(60_000)
    })
    expect(statusText(view)).toBe('firing…')
  })

  it('a later fire shows the daemon calendar label', () => {
    const view = renderRow(
      entry({
        frequency: 'daily',
        specJson: JSON.stringify({ frequency: 'daily', time: '07:00' }),
        waitReason: 'scheduled',
        nextFireAt: iso(new Date(2026, 9, 8, 7, 0).getTime()),
      }),
    )
    expect(statusText(view)).toBe('Next run: Tomorrow 7:00 AM')
  })

  it('an overdue row shows how late and the gate', () => {
    const view = renderRow(
      entry({
        waitReason: 'overdue',
        waitDetail: 'not_fired: the scheduler is ticking but has not fired this slot',
        nextFireAt: iso(NOW - 6 * MIN),
      }),
    )
    expect(statusText(view)).toBe('overdue 6m: scheduler ticking, slot not fired')
  })

  it('a server that stopped ticking says so, with how long', () => {
    const view = renderRow(
      entry({
        waitReason: 'no_ticks',
        waitDetail: 'no scheduler tick since x',
        waitSince: iso(NOW - 8 * HOUR),
        nextFireAt: iso(NOW - 7 * HOUR),
      }),
    )
    expect(statusText(view)).toBe('waiting: scheduler not ticking (8h)')
  })

  it('every wait_reason the daemon sends renders text, never "now" or "0m 0s"', () => {
    for (const reason of REASONS) {
      const disabled = reason.startsWith('disabled:')
      for (const next of [iso(NOW - 3 * MIN), iso(NOW), null]) {
        const view = renderRow(entry({ enabled: !disabled, waitReason: reason, nextFireAt: next }))
        const text = statusText(view)
        expect(text.length, `${reason} @ ${next}`).toBeGreaterThan(0)
        expect(text, `${reason} @ ${next}`).not.toMatch(/\bnow\b/)
        expect(text, `${reason} @ ${next}`).not.toMatch(/0m 0s|0m 00s/)
        expect(text, `${reason} @ ${next}`).not.toBe(`waiting: ${reason}`)
        cleanup()
      }
    }
  })
})

describe('S5 copy is kept', () => {
  it('an empty WAKEUP.md row stays enabled and says it is waiting', () => {
    const view = renderRow(entry({ waitReason: 'wakeup_empty', waitDetail: 'needs instructions', nextFireAt: iso(NOW + HOUR) }))
    expect(statusText(view)).toBe('waiting: WAKEUP.md is empty')
    expect(view.queryByText(/Next run/)).toBeNull()
    expect(view.getByRole('switch').getAttribute('aria-checked')).toBe('true')
  })

  it("Reggie's row reads invalid schedule with the daemon's detail", () => {
    const view = renderRow(
      entry({
        frequency: 'list',
        specJson: '{}',
        waitReason: 'schedule_error',
        waitDetail: "unknown frequency 'list'",
        scheduleError: "unknown frequency 'list'",
      }),
    )
    expect(statusText(view)).toBe("invalid schedule: unknown frequency 'list'")
  })
})

describe('HB30: a server older than heartbeat-next-fire', () => {
  it('shows the schedule text only: no countdown, no "now"', () => {
    // A stray nextFireAt from such a server is never shown.
    const view = renderRow(entry({ lastFired: iso(NOW - 20 * MIN), nextFireAt: iso(NOW - MIN) }), scopeOld)
    expect(view.getByText('Every 15m')).toBeTruthy()
    expect(view.container.querySelector('[data-heartbeat-status]')).toBeNull()
    expect(view.queryByText(/Next run|now|firing/)).toBeNull()
  })

  it('still names a stored schedule error', () => {
    const view = renderRow(entry({ scheduleError: 'bad spec' }), scopeOld)
    expect(statusText(view)).toBe('invalid schedule: bad spec')
  })
})

describe('HB26: the guesswork is gone', () => {
  it('HeartbeatEntry.tsx has no schedule math and no "now" literal', () => {
    const src = readFileSync(join(ROOT, 'src/renderer/components/HeartbeatsPanel/HeartbeatEntry.tsx'), 'utf8')
    expect(src).not.toContain('describeNextRun')
    expect(src).not.toContain('matchesScheduleDay')
    expect(src).not.toMatch(/return 'now'/)
    expect(src).toContain('<HeartbeatStatusLine')
  })
})
