import { describe, it, expect, vi } from 'vitest'
import { createContextLossTracker } from './contextLoss'

/** Manual timer harness: fire() runs the pending timeout. */
function harness(timeoutMs = 3000) {
  const onRestore = vi.fn()
  const onFatal = vi.fn()
  let pending: (() => void) | null = null
  let cleared = 0
  const tracker = createContextLossTracker({
    onRestore,
    onFatal,
    timeoutMs,
    setTimer: (fn) => {
      pending = fn
      return 1
    },
    clearTimer: () => {
      cleared++
      pending = null
    },
  })
  return {
    tracker,
    onRestore,
    onFatal,
    fire: () => {
      const fn = pending
      pending = null
      fn?.()
    },
    get cleared() {
      return cleared
    },
    get hasPending() {
      return pending !== null
    },
  }
}

describe('context-loss tracker', () => {
  it('restores within the window: timer cleared, onRestore fires once', () => {
    const h = harness()
    h.tracker.handleLost()
    expect(h.tracker.state).toBe('lost')
    expect(h.hasPending).toBe(true)
    h.tracker.handleRestored()
    expect(h.tracker.state).toBe('live')
    expect(h.onRestore).toHaveBeenCalledTimes(1)
    expect(h.onFatal).not.toHaveBeenCalled()
    expect(h.cleared).toBe(1)
  })

  it('window expiry goes fatal permanently', () => {
    const h = harness()
    h.tracker.handleLost()
    h.fire()
    expect(h.tracker.state).toBe('fatal')
    expect(h.onFatal).toHaveBeenCalledWith('webgl-context-lost-unrestored')
    // A late restore event must NOT resurrect the painter — the pane
    // already fell back to DOM.
    h.tracker.handleRestored()
    expect(h.tracker.state).toBe('fatal')
    expect(h.onRestore).not.toHaveBeenCalled()
  })

  it('survives repeated lost/restored cycles', () => {
    const h = harness()
    h.tracker.handleLost()
    h.tracker.handleRestored()
    h.tracker.handleLost()
    h.tracker.handleRestored()
    expect(h.tracker.state).toBe('live')
    expect(h.onRestore).toHaveBeenCalledTimes(2)
  })

  it('duplicate lost events do not restart the timer', () => {
    const h = harness()
    h.tracker.handleLost()
    const before = h.hasPending
    h.tracker.handleLost()
    expect(before).toBe(true)
    // Only one restore needed to come back.
    h.tracker.handleRestored()
    expect(h.tracker.state).toBe('live')
  })

  it('spurious restored while live is a no-op', () => {
    const h = harness()
    h.tracker.handleRestored()
    expect(h.tracker.state).toBe('live')
    expect(h.onRestore).not.toHaveBeenCalled()
  })

  it('dispose clears a pending timer', () => {
    const h = harness()
    h.tracker.handleLost()
    h.tracker.dispose()
    expect(h.hasPending).toBe(false)
  })
})
