// Unit tests for the shared backoff jitter (0.40.48 connection resilience).
//
// The invariants the retry loops rely on:
//   - fullJitter(ms) ∈ [0, ms] — maximum decorrelation.
//   - jittered(ms, f) ∈ [ms·(1−f), ms] — spreads retries WITHOUT exceeding
//     the schedule's caps or collapsing a meaningful delay to ~0.
//   - 0 stays 0 in both — the "immediate first retry" that evicts a dead
//     pooled WKWebView socket must remain immediate.

import { describe, it, expect } from 'vitest'
import { fullJitter, jittered } from './backoff'

const SAMPLES = 500

describe('fullJitter', () => {
  it('stays within [0, ms]', () => {
    for (let i = 0; i < SAMPLES; i++) {
      const v = fullJitter(8000)
      expect(v).toBeGreaterThanOrEqual(0)
      expect(v).toBeLessThanOrEqual(8000)
    }
  })

  it('0 (and negatives) stay 0 — immediate retries remain immediate', () => {
    expect(fullJitter(0)).toBe(0)
    expect(fullJitter(-100)).toBe(0)
  })

  it('actually varies (not a constant)', () => {
    const seen = new Set<number>()
    for (let i = 0; i < SAMPLES; i++) seen.add(fullJitter(8000))
    expect(seen.size).toBeGreaterThan(1)
  })
})

describe('jittered', () => {
  it('default factor 0.5 stays within [ms/2, ms] — caps hold, floor holds', () => {
    for (let i = 0; i < SAMPLES; i++) {
      const v = jittered(8000)
      expect(v).toBeGreaterThanOrEqual(4000)
      expect(v).toBeLessThanOrEqual(8000)
    }
  })

  it('a custom factor narrows the band', () => {
    for (let i = 0; i < SAMPLES; i++) {
      const v = jittered(1000, 0.1)
      expect(v).toBeGreaterThanOrEqual(900)
      expect(v).toBeLessThanOrEqual(1000)
    }
  })

  it('never EXCEEDS the base delay (the 8s recovery cap must survive jitter)', () => {
    for (let i = 0; i < SAMPLES; i++) {
      expect(jittered(8000)).toBeLessThanOrEqual(8000)
    }
  })

  it('0 (and negatives) stay 0 — remote-retry delay 0 semantics survive', () => {
    expect(jittered(0)).toBe(0)
    expect(jittered(-5)).toBe(0)
  })

  it('actually varies (not a constant)', () => {
    const seen = new Set<number>()
    for (let i = 0; i < SAMPLES; i++) seen.add(jittered(8000))
    expect(seen.size).toBeGreaterThan(1)
  })
})
