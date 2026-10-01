// Heartbeat S4 (prd-heartbeat-firing-v1 HB26/HB27/HB29/HB30/HB31): the one
// formatter. Copy comes from the daemon's `nextFireAt` + `waitReason`
// only. Fail loud — no skips, no `??` defaults in assertions.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  FIRING_GRACE_MS,
  MAX_TIMER_MS,
  REFETCH_SLACK_MS,
  describeHeartbeatStatus,
  heartbeatStatusText,
  nextDueRefetchDelay,
  type HeartbeatWaitFields,
} from './heartbeat-wait'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '../../..')
/** The daemon's HB20 vocabulary (S3 T-S3f writes it from Rust). */
const REASONS: string[] = JSON.parse(
  readFileSync(join(ROOT, 'crates/k2-core/src/heartbeats/fixtures/wait-reasons.json'), 'utf8'),
)

// A fixed local "now": Wed 2026-10-07 14:00:00 local time.
const NOW = new Date(2026, 9, 7, 14, 0, 0).getTime()
const iso = (t: number): string => new Date(t).toISOString()
const MIN = 60_000
const HOUR = 3_600_000
const on = { now: NOW, nextFire: true }

function text(row: HeartbeatWaitFields, opts = on): string {
  const t = heartbeatStatusText(row, opts)
  if (t === null) throw new Error(`expected status text for ${JSON.stringify(row)}`)
  return t
}

/** Every reason with the fields the daemon sends alongside it. */
function rowFor(reason: string, nextFireAt: string | null): HeartbeatWaitFields {
  const disabled = reason.startsWith('disabled:')
  const detail: Record<string, string> = {
    window_closed: 'window opens 22:00',
    backoff: 'failure 2 of 5',
    schedule_error: "unknown frequency 'list'",
    overdue: 'no_ticks: no scheduler tick since 2026-10-07T12:00:00Z',
    no_ticks: 'no scheduler tick since 2026-10-07T06:00:00Z',
  }
  return {
    enabled: !disabled,
    waitReason: reason,
    waitDetail: detail[reason] ?? null,
    waitSince: reason === 'no_ticks' ? iso(NOW - 8 * HOUR) : null,
    nextFireAt,
  }
}

describe('the vocabulary contract (T-S3f, renderer half)', () => {
  it('the fixture is the daemon list and non-empty', () => {
    expect(REASONS.length).toBe(14)
    expect(REASONS).toContain('no_ticks')
  })

  it('every reason the daemon can send has its own copy, never the unknown-reason fallback', () => {
    for (const reason of REASONS) {
      for (const next of [iso(NOW + 30 * MIN), iso(NOW - 6 * MIN), null]) {
        const t = text(rowFor(reason, next))
        expect(t, `${reason} @ ${next}`).not.toBe(`waiting: ${reason}`)
        expect(t, `${reason} @ ${next}`).not.toMatch(new RegExp(`^waiting: ${reason}( \\(|$)`))
      }
    }
  })

  it('no reason, at any time, renders a bare "now" or "0m 0s"', () => {
    for (const reason of REASONS) {
      for (const offset of [-8 * HOUR, -6 * MIN, -FIRING_GRACE_MS, -1000, 0, 400, 1000, 59 * MIN]) {
        const t = text(rowFor(reason, iso(NOW + offset)))
        expect(t, `${reason} @ ${offset}`).not.toMatch(/^now$/)
        expect(t, `${reason} @ ${offset}`).not.toMatch(/\bnow\b/)
        expect(t, `${reason} @ ${offset}`).not.toMatch(/0m 0s|0m 00s/)
      }
    }
  })
})

describe('each wait_reason renders its text', () => {
  it('scheduled: a countdown under an hour, a calendar label beyond', () => {
    expect(text(rowFor('scheduled', iso(NOW + 14 * MIN + 50_000)))).toBe('in 14m 50s')
    expect(text(rowFor('scheduled', iso(NOW + 45_000)))).toBe('in 45s')
    expect(text(rowFor('scheduled', iso(NOW + 400)))).toBe('in 1s')
    expect(text(rowFor('scheduled', iso(new Date(2026, 9, 7, 19, 30).getTime())))).toBe('Today 7:30 PM')
    expect(text(rowFor('scheduled', iso(new Date(2026, 9, 8, 7, 0).getTime())))).toBe('Tomorrow 7:00 AM')
    expect(text(rowFor('scheduled', iso(new Date(2026, 9, 10, 9, 0).getTime())))).toBe('Sat 9:00 AM')
    expect(text(rowFor('scheduled', iso(new Date(2026, 10, 2, 9, 0).getTime())))).toBe('Nov 2 9:00 AM')
  })

  it('scheduled at or past its time reads "firing…" for 120 s, then overdue', () => {
    expect(text(rowFor('scheduled', iso(NOW)))).toBe('firing…')
    expect(text(rowFor('scheduled', iso(NOW - FIRING_GRACE_MS)))).toBe('firing…')
    expect(text(rowFor('scheduled', iso(NOW - FIRING_GRACE_MS - 1000)))).toBe('overdue 2m')
  })

  it('a missed run the daemon is catching up', () => {
    const row = { ...rowFor('scheduled', iso(NOW - 3 * HOUR)), waitDetail: 'missed slot; the next tick fires one catch-up' }
    expect(text(row)).toBe('catching up a missed run')
  })

  it('in_flight', () => {
    expect(text(rowFor('in_flight', iso(NOW - MIN)))).toBe('firing…')
  })

  it('window_closed names when the window opens', () => {
    expect(text(rowFor('window_closed', iso(new Date(2026, 9, 7, 22, 0).getTime())))).toBe(
      'waiting: window opens 10:00 PM',
    )
    expect(text(rowFor('window_closed', iso(new Date(2026, 9, 8, 6, 0).getTime())))).toBe(
      'waiting: window opens Tomorrow 6:00 AM',
    )
    expect(text(rowFor('window_closed', null))).toBe('waiting: firing window closed')
  })

  it('backoff counts down to the retry and names the failure', () => {
    expect(text(rowFor('backoff', iso(NOW + 2 * MIN)))).toBe('retry in 2m (failure 2 of 5)')
    expect(text(rowFor('backoff', iso(NOW + 30_000)))).toBe('retry in 30s (failure 2 of 5)')
  })

  it('schedule_error, with the daemon detail or the stored error', () => {
    expect(text(rowFor('schedule_error', null))).toBe("invalid schedule: unknown frequency 'list'")
    expect(text({ enabled: true, scheduleError: 'bad spec' })).toBe('invalid schedule: bad spec')
  })

  it('wakeup_empty, no_agent, no_project', () => {
    expect(text(rowFor('wakeup_empty', iso(NOW + HOUR)))).toBe('waiting: WAKEUP.md is empty')
    expect(text(rowFor('no_agent', iso(NOW - HOUR)))).toBe('waiting: no agent in this workspace')
    expect(text(rowFor('no_project', iso(NOW - HOUR)))).toBe('waiting: workspace folder not found')
  })

  it('no_ticks says how long the scheduler has been quiet', () => {
    expect(text(rowFor('no_ticks', iso(NOW - 6 * HOUR)))).toBe('waiting: scheduler not ticking (8h)')
    expect(text({ ...rowFor('no_ticks', iso(NOW - HOUR)), waitSince: null })).toBe('waiting: scheduler not ticking')
  })

  it('overdue names how late and the gate', () => {
    expect(text(rowFor('overdue', iso(NOW - 6 * MIN)))).toBe('overdue 6m: scheduler not ticking')
    expect(
      text({ ...rowFor('overdue', iso(NOW - 6 * MIN)), waitDetail: 'not_fired: the scheduler is ticking but has not fired this slot' }),
    ).toBe('overdue 6m: scheduler ticking, slot not fired')
  })

  it('disabled:* says why', () => {
    expect(text(rowFor('disabled:user', null))).toBe('disabled')
    expect(text(rowFor('disabled:failures', null))).toBe('disabled: repeated failures')
    expect(text(rowFor('disabled:wakeup_missing', null))).toBe('disabled: WAKEUP.md missing')
    expect(text(rowFor('disabled:wakeup_empty', null))).toBe('disabled: WAKEUP.md is empty')
    // A server older than S3: `disabledReason` only.
    expect(text({ enabled: false, disabledReason: 'failures' })).toBe('disabled: repeated failures')
  })

  it('a reason this client has no copy for is said, not hidden', () => {
    expect(text({ enabled: true, waitReason: 'something_new', waitDetail: 'x' })).toBe('waiting: something_new (x)')
  })
})

describe('a server without heartbeat-next-fire (HB30)', () => {
  const off = { now: NOW, nextFire: false }

  it('never shows a time, even when a stray nextFireAt arrives', () => {
    expect(heartbeatStatusText({ enabled: true, nextFireAt: iso(NOW + 10 * MIN) }, off)).toBeNull()
    expect(heartbeatStatusText({ enabled: true, nextFireAt: iso(NOW - HOUR) }, off)).toBeNull()
  })

  it('still names a stored schedule error or a disable reason', () => {
    expect(heartbeatStatusText({ enabled: true, scheduleError: 'bad' }, off)).toBe('invalid schedule: bad')
    expect(heartbeatStatusText({ enabled: false, disabledReason: 'wakeup_missing' }, off)).toBe(
      'disabled: WAKEUP.md missing',
    )
  })
})

describe('ticking', () => {
  it('a countdown ticks; a static reason does not', () => {
    const next = describeHeartbeatStatus(rowFor('scheduled', iso(NOW + MIN)), on)
    const empty = describeHeartbeatStatus(rowFor('wakeup_empty', iso(NOW + MIN)), on)
    if (!next || !empty) throw new Error('expected both statuses')
    expect(next.ticking).toBe(true)
    expect(next.tone).toBe('next')
    expect(empty.ticking).toBe(false)
  })
})

describe('nextDueRefetchDelay (HB29)', () => {
  it('is the earliest future nextFireAt + 120 s across enabled rows', () => {
    const rows = [
      { enabled: true, nextFireAt: iso(NOW + 10 * MIN) },
      { enabled: true, nextFireAt: iso(NOW + 2 * MIN) },
      { enabled: false, nextFireAt: iso(NOW + MIN) },
      { enabled: true, nextFireAt: iso(NOW + 30_000), archivedAt: '2026-10-01T00:00:00Z' },
      { enabled: true, nextFireAt: null },
    ]
    expect(nextDueRefetchDelay(rows, NOW)).toBe(2 * MIN + FIRING_GRACE_MS + REFETCH_SLACK_MS)
  })

  it('a deadline already behind now never re-arms', () => {
    expect(nextDueRefetchDelay([{ enabled: true, nextFireAt: iso(NOW - 10 * MIN) }], NOW)).toBeNull()
    expect(nextDueRefetchDelay([], NOW)).toBeNull()
  })

  it('a far deadline is clamped to what setTimeout honours', () => {
    expect(nextDueRefetchDelay([{ enabled: true, nextFireAt: iso(NOW + 300 * 24 * HOUR) }], NOW)).toBe(MAX_TIMER_MS)
  })
})
