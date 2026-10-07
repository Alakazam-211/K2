// prd-daemon-activity-and-thread-working-v1 DA31 / RL14: every reason the
// daemon can send has copy, and the display labels read as RL14 says.
// The vocabulary comes from the daemon's own fixture (a Rust test writes
// it), so a reason added there without copy here fails. Fail loud.

import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { DISPLAY_LABEL, REASON_COPY, activityLabel, activityTitle, minutesWithoutUpdate } from './activity-copy'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '../../..')
const REASONS: string[] = JSON.parse(
  readFileSync(join(ROOT, 'crates/k2-core/src/activity/fixtures/reasons.json'), 'utf8'),
)
const MIN = 60_000

describe('activity copy (DA31, RL14)', () => {
  it('the fixture is the daemon vocabulary', () => {
    expect(REASONS.length).toBeGreaterThanOrEqual(28)
    expect(REASONS).toContain('stale_no_evidence')
  })

  it('every daemon reason has copy, and no copy is for a reason the daemon dropped', () => {
    const missing = REASONS.filter((r) => typeof REASON_COPY[r] !== 'string' || REASON_COPY[r].length === 0)
    expect(missing).toEqual([])
    expect(Object.keys(REASON_COPY).sort()).toEqual([...REASONS].sort())
  })

  it('labels each display (RL14)', () => {
    expect(DISPLAY_LABEL).toEqual({
      working: 'working',
      monitoring: 'monitoring background tasks',
      waiting: 'needs you',
      unverifiable: 'no update',
      idle: 'idle',
    })
    expect(activityLabel({ display: 'waiting', evidenceAt: 0, staleSince: null }, 0)).toBe('needs you')
  })

  it('unverifiable says how long nothing has been heard, from the last evidence', () => {
    const now = 100 * MIN
    expect(activityLabel({ display: 'unverifiable', evidenceAt: now - 34 * MIN, staleSince: now - 4 * MIN }, now)).toBe(
      'no update in 34m',
    )
    // No evidence time: back it out of staleSince (STALE_AFTER = 30 m).
    expect(minutesWithoutUpdate({ evidenceAt: null, staleSince: now - 4 * MIN }, now)).toBe(34)
    // The server clock skew is applied.
    expect(minutesWithoutUpdate({ evidenceAt: now - 34 * MIN, staleSince: null }, now, 2 * MIN)).toBe(36)
  })

  it('the tooltip is the label, then why', () => {
    expect(
      activityTitle({ display: 'monitoring', evidenceAt: 0, staleSince: null, reason: 'background_shell' }, 0),
    ).toBe('Monitoring background tasks · A background command is running')
  })
})
