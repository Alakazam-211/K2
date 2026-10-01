// Heartbeat S2 (D2, D8, W8) — the Settings copy matches what the daemon
// does: it ticks itself, "Off" only means "don't wake", there is one
// switch, and nothing claims heartbeats need the app open or launchd.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const src = readFileSync(
  join(__dirname, 'WakeSchedulerSection.tsx'),
  'utf8',
)

describe('wake scheduler copy (S2)', () => {
  it('has one wake switch and the battery checkbox', () => {
    expect(src).toContain('Wake this computer for heartbeats')
    expect(src).toContain('Also on battery')
    expect(src).toContain("'heartbeat/wake'")
  })

  it('never says heartbeats need the app open, launchd, or a plist', () => {
    expect(src).not.toMatch(/while K2 is open/i)
    expect(src).not.toMatch(/launchd/)
    expect(src).not.toMatch(/WakeSystem/)
    expect(src).not.toMatch(/\.plist/)
    expect(src).not.toContain('apply-wake-scheduler')
  })

  it('says Off still fires while awake and names the 12 h catch-up', () => {
    expect(src).toContain('Heartbeats still fire whenever it is awake.')
    expect(src).toContain('less than 12 hours late')
  })
})
