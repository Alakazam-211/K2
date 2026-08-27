import { describe, expect, it } from 'vitest'

import {
  ACK_PROBE_MS,
  shouldLogGridStallRecovered,
  tickGridHealth,
  WS_CONNECTING,
  WS_OPEN,
} from './gridHealthTick'

describe('grid health tick (G2/G4/G5)', () => {
  it('OPEN idle ≥20s does not resync, stall-warn, or recovered-warn; probe still allowed', () => {
    const lastFrameAt = 1_000
    const now = lastFrameAt + 20_000
    const tick = tickGridHealth({
      now,
      readyState: WS_OPEN,
      lastFrameAt,
      lastAckVersion: 9,
      lastAckProbeAt: 0,
      stallActive: false,
    })
    expect(tick.stallWarnReason).toBe(null)
    expect(tick.stallActive).toBe(false)
    expect(shouldLogGridStallRecovered(tick.stallActive)).toBe(false)
    // 15s ack probe is G2 — still send last version. No grid-resync.
    expect(tick.ackProbeVersion).toBe(9)
    expect(now - lastFrameAt).toBeGreaterThanOrEqual(ACK_PROBE_MS)
  })

  it('OPEN idle does not latch stall even at 60s of silence', () => {
    const tick = tickGridHealth({
      now: 61_000,
      readyState: WS_OPEN,
      lastFrameAt: 1_000,
      lastAckVersion: 3,
      lastAckProbeAt: 50_000,
      stallActive: false,
    })
    expect(tick.stallActive).toBe(false)
    expect(tick.stallWarnReason).toBe(null)
    expect(shouldLogGridStallRecovered(false)).toBe(false)
  })

  it('not-OPEN missing WS warns once as ws-missing', () => {
    const tick = tickGridHealth({
      now: 8_000,
      readyState: null,
      lastFrameAt: 1_000,
      lastAckVersion: 1,
      lastAckProbeAt: 0,
      stallActive: false,
    })
    expect(tick.stallWarnReason).toBe('ws-missing')
    expect(tick.stallActive).toBe(true)
    expect(tick.ackProbeVersion).toBe(null)
    expect(shouldLogGridStallRecovered(tick.stallActive)).toBe(true)
  })

  it('not-OPEN CLOSING warns as ws-not-open and does not ack-probe', () => {
    const tick = tickGridHealth({
      now: 8_000,
      readyState: 2,
      lastFrameAt: 1_000,
      lastAckVersion: 4,
      lastAckProbeAt: 0,
      stallActive: false,
    })
    expect(tick.stallWarnReason).toBe('ws-not-open:2')
    expect(tick.stallActive).toBe(true)
    expect(tick.ackProbeVersion).toBe(null)
  })

  it('CONNECTING does not stall-warn', () => {
    const tick = tickGridHealth({
      now: 8_000,
      readyState: WS_CONNECTING,
      lastFrameAt: 1_000,
      lastAckVersion: 1,
      lastAckProbeAt: 0,
      stallActive: false,
    })
    expect(tick.stallWarnReason).toBe(null)
    expect(tick.stallActive).toBe(false)
  })

  it('does not reintroduce grid-stall-no-frame as a heal action', () => {
    const tick = tickGridHealth({
      now: 25_000,
      readyState: WS_OPEN,
      lastFrameAt: 1_000,
      lastAckVersion: 2,
      lastAckProbeAt: 0,
      stallActive: false,
    })
    expect(tick.stallWarnReason).toBe(null)
    expect(JSON.stringify(tick)).not.toMatch(/grid-stall-no-frame/)
  })
})
