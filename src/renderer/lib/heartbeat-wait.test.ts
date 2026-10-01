// S5 (prd-heartbeat-firing-v1 HB27/HB31): the shared wait formatter.
// Copy comes from the daemon's reason only. Fail loud — no skips.
import { describe, expect, it } from 'vitest'
import { describeHeartbeatWait } from './heartbeat-wait'

describe('describeHeartbeatWait', () => {
  it('says an enabled row with an empty WAKEUP.md is waiting', () => {
    expect(describeHeartbeatWait({ enabled: true, waitReason: 'wakeup_empty' })).toBe(
      'waiting: WAKEUP.md is empty',
    )
  })

  it("shows Reggie's invalid schedule with the daemon's detail", () => {
    expect(
      describeHeartbeatWait({
        enabled: true,
        waitReason: 'schedule_error',
        waitDetail: "unknown frequency 'list'",
        scheduleError: "unknown frequency 'list'",
      }),
    ).toBe("invalid schedule: unknown frequency 'list'")
  })

  it('falls back to the stored scheduleError from a daemon older than S5', () => {
    expect(describeHeartbeatWait({ enabled: true, scheduleError: 'bad spec' })).toBe(
      'invalid schedule: bad spec',
    )
  })

  it('names why a row was disabled', () => {
    expect(describeHeartbeatWait({ enabled: false, disabledReason: 'wakeup_empty' })).toBe(
      'disabled: WAKEUP.md is empty',
    )
    expect(describeHeartbeatWait({ enabled: false, disabledReason: 'wakeup_missing' })).toBe(
      'disabled: WAKEUP.md missing',
    )
    expect(describeHeartbeatWait({ enabled: false, disabledReason: 'failures' })).toBe(
      'disabled: repeated failures',
    )
    expect(describeHeartbeatWait({ enabled: false, disabledReason: null })).toBeNull()
  })

  it('returns null when nothing is in the way, and never says "now"', () => {
    expect(describeHeartbeatWait({ enabled: true })).toBeNull()
    for (const waitReason of ['wakeup_empty', 'schedule_error']) {
      const text = describeHeartbeatWait({ enabled: true, waitReason, waitDetail: 'x' })
      expect(text).not.toBeNull()
      expect(text).not.toMatch(/^now$/)
      expect(text).not.toMatch(/0m 0s/)
    }
  })
})
