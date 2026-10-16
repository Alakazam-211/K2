// Fail loud: freeze shows a picture before hiding, never blocks past the
// budget, and never leaks or shows a stale picture.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { BrowserFreeze, type BrowserFreezeDeps, type FrozenFrame } from './browserFreeze'

interface Harness {
  freeze: BrowserFreeze
  log: string[]
  shown: (FrozenFrame | null)[]
  resolveSnapshot: (frame: FrozenFrame | null) => void
  rejectSnapshot: (err: unknown) => void
  snapshotCalls: () => number
  paintQueue: (() => void)[]
  released: string[]
}

function frame(name: string, released: string[]): FrozenFrame {
  return { url: `blob:${name}`, width: 600, height: 400, release: () => released.push(name) }
}

function harness(initial: { face: 'live' | 'frozen' | 'hidden'; nativeShown: boolean }): Harness {
  const log: string[] = []
  const shown: (FrozenFrame | null)[] = []
  const paintQueue: (() => void)[] = []
  const released: string[] = []
  let resolveSnapshot: (f: FrozenFrame | null) => void = () => {
    throw new Error('no snapshot pending')
  }
  let rejectSnapshot: (e: unknown) => void = () => {
    throw new Error('no snapshot pending')
  }
  let calls = 0
  const deps: BrowserFreezeDeps = {
    budgetMs: 150,
    snapshot: () => {
      calls++
      log.push('snapshot')
      return new Promise<FrozenFrame | null>((res, rej) => {
        resolveSnapshot = res
        rejectSnapshot = rej
      })
    },
    setNativeVisible: (v) => log.push(v ? 'show' : 'hide'),
    beforeShow: () => log.push('bounds'),
    showFrame: (f) => {
      shown.push(f)
      log.push(f ? `frame:${f.url}` : 'frame:none')
    },
    afterPaint: (fn) => paintQueue.push(fn),
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  }
  return {
    freeze: new BrowserFreeze(deps, initial),
    log,
    shown,
    resolveSnapshot: (f) => resolveSnapshot(f),
    rejectSnapshot: (e) => rejectSnapshot(e),
    snapshotCalls: () => calls,
    paintQueue,
    released,
  }
}

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await Promise.resolve()
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('BrowserFreeze', () => {
  it('covered: shows the picture first, then hides the native view', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    expect(h.log).toEqual(['snapshot'])
    h.resolveSnapshot(frame('a', h.released))
    await settle()
    expect(h.log).toEqual(['snapshot', 'frame:blob:a', 'hide'])
    // The budget timer was cancelled: no second hide.
    vi.advanceTimersByTime(1000)
    expect(h.log.filter((l) => l === 'hide')).toHaveLength(1)
  })

  it('uncovered: bounds, show, then drops the picture after a paint', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    h.resolveSnapshot(frame('a', h.released))
    await settle()
    h.log.length = 0
    h.freeze.set('live')
    expect(h.log).toEqual(['bounds', 'show'])
    expect(h.paintQueue).toHaveLength(1)
    h.paintQueue.shift()?.()
    expect(h.log).toEqual(['bounds', 'show', 'frame:none'])
  })

  it('a slow snapshot falls back to the blank hide at the budget, and a late picture still fills it', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    vi.advanceTimersByTime(149)
    expect(h.log).toEqual(['snapshot'])
    vi.advanceTimersByTime(1)
    expect(h.log).toEqual(['snapshot', 'hide'])
    h.resolveSnapshot(frame('late', h.released))
    await settle()
    expect(h.log).toEqual(['snapshot', 'hide', 'frame:blob:late'])
  })

  it('no picture (stub, no view, error) hides at once', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    h.resolveSnapshot(null)
    await settle()
    expect(h.log).toEqual(['snapshot', 'hide'])

    const e = harness({ face: 'live', nativeShown: true })
    e.freeze.set('frozen')
    e.rejectSnapshot(new Error('no such browser view'))
    await settle()
    expect(e.log).toEqual(['snapshot', 'hide'])
  })

  it('an overlay that closes before the snapshot lands: no hide, the picture is released unseen', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    h.freeze.set('live')
    h.resolveSnapshot(frame('stale', h.released))
    await settle()
    vi.advanceTimersByTime(1000)
    expect(h.log).toEqual(['snapshot', 'bounds'])
    expect(h.shown).toEqual([])
    expect(h.released).toEqual(['stale'])
  })

  it('hidden (tab switch, Settings) hides and drops any picture; no snapshot', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    h.resolveSnapshot(frame('a', h.released))
    await settle()
    h.log.length = 0
    h.freeze.set('hidden')
    expect(h.log).toEqual(['frame:none'])
    const fresh = harness({ face: 'live', nativeShown: true })
    fresh.freeze.set('hidden')
    expect(fresh.log).toEqual(['hide'])
    expect(fresh.snapshotCalls()).toBe(0)
  })

  it('hidden → covered: nothing to photograph, stays hidden', () => {
    const h = harness({ face: 'hidden', nativeShown: false })
    h.freeze.set('frozen')
    expect(h.log).toEqual([])
    expect(h.snapshotCalls()).toBe(0)
    h.freeze.set('live')
    expect(h.log).toEqual(['bounds', 'show'])
  })

  it('a view created while covered freezes right away; one created while hidden hides', async () => {
    const covered = harness({ face: 'frozen', nativeShown: false })
    covered.freeze.adoptCreatedView()
    expect(covered.log).toEqual(['snapshot'])
    covered.resolveSnapshot(frame('new', covered.released))
    await settle()
    expect(covered.log).toEqual(['snapshot', 'frame:blob:new', 'hide'])

    const hidden = harness({ face: 'hidden', nativeShown: false })
    hidden.freeze.adoptCreatedView()
    expect(hidden.log).toEqual(['hide'])

    const live = harness({ face: 'live', nativeShown: false })
    live.freeze.adoptCreatedView()
    expect(live.log).toEqual([])
  })

  it('re-covering before the old picture is dropped replaces it with a new one', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    h.resolveSnapshot(frame('one', h.released))
    await settle()
    h.freeze.set('live')
    h.freeze.set('frozen')
    // The pending drop from the first uncover is now stale.
    h.paintQueue.shift()?.()
    h.resolveSnapshot(frame('two', h.released))
    await settle()
    expect(h.shown.map((f) => f?.url ?? null)).toEqual(['blob:one', 'blob:two'])
    expect(h.log.at(-1)).toBe('hide')
  })

  it('dispose drops the picture and cancels a pending snapshot', async () => {
    const h = harness({ face: 'live', nativeShown: true })
    h.freeze.set('frozen')
    h.freeze.dispose()
    h.resolveSnapshot(frame('gone', h.released))
    await settle()
    vi.advanceTimersByTime(1000)
    expect(h.log).toEqual(['snapshot'])
    expect(h.released).toEqual(['gone'])
  })
})
