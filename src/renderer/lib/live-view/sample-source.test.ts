// SampleWorkbookSource plays the goldens on a timer: play, pause, step,
// loop (a reset, then the welcome again) or end cleanly; close stops it all.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { SampleWorkbookSource, sampleSteps } from './sample-source'
import type { LiveWorkbookSourceEvent } from './source'

describe('sampleSteps', () => {
  it('the turn: welcome, snapshot pages, roster, then every golden frame in order with its flush', () => {
    const steps = sampleSteps('turn')
    const types = steps.flatMap((s) => s.events.map((e) => e.type))
    expect(types[0]).toBe('welcome')
    expect(types.filter((t) => t === 'keyed_page').length).toBe(11)
    const revs = steps.flatMap((s) => s.events.flatMap((e) => (e.type === 'ops_change' ? [e.rev] : [])))
    expect(revs).toEqual([21, 22, 23, 24, 25, 26, 27, 28, 29])
    // Each change carries the next seq after the snapshot's pages, with no gap.
    const seqs = steps.flatMap((s) => s.events.flatMap((e) => (e.type === 'keyed_snapshot' || e.type === 'keyed_page' || e.type === 'ops_change' ? [e.seq] : [])))
    expect(seqs).toEqual(seqs.map((_, i) => i + 1))
    // The flush after rev 21 lands in the same step as its frame.
    const s21 = steps.find((s) => s.label === 'rev 21')
    expect(s21?.events.map((e) => (e.type === 'roster_event' ? e.event.kind : e.type))).toEqual(['ops_change', 'focus', 'selection', 'cells'])
  })
  it('too large: the welcome, the blob tier, the snapshot; no roster, no changes', () => {
    const types = sampleSteps('blob').flatMap((s) => s.events.map((e) => (e.type === 'tier' ? `tier:${e.tier}` : e.type)))
    expect(types.slice(0, 2)).toEqual(['welcome', 'tier:blob'])
    expect(types).not.toContain('roster_event')
    expect(types).not.toContain('ops_change')
  })
  it('no doc: the welcome and the none tier only', () => {
    expect(sampleSteps('none').flatMap((s) => s.events.map((e) => e.type))).toEqual(['welcome', 'tier'])
  })
  it('no annotation reaches the wire', () => {
    const json = JSON.stringify(sampleSteps('turn'))
    expect(json).not.toMatch(/"_[a-z]/)
  })
})

describe('SampleWorkbookSource', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  const collect = (src: SampleWorkbookSource): LiveWorkbookSourceEvent[] => {
    const got: LiveWorkbookSourceEvent[] = []
    src.subscribe((e) => got.push(e))
    return got
  }

  it('plays on its own and stops at the end without looping', () => {
    const src = new SampleWorkbookSource({ loop: false })
    const got = collect(src)
    vi.advanceTimersByTime(120_000)
    expect(got.filter((e) => e.type === 'ops_change').length).toBe(9)
    expect(src.status()).toMatchObject({ playing: false, ended: true })
    expect(src.snapshot().length).toBe(got.length)
    src.close()
  })

  it('pause holds; step sends exactly one step; play goes on', () => {
    const src = new SampleWorkbookSource({ autoplay: false, loop: false })
    const got = collect(src)
    vi.advanceTimersByTime(10_000)
    expect(got).toEqual([])
    src.step()
    expect(got.map((e) => e.type)).toEqual(['welcome', 'tier'])
    expect(src.status()).toMatchObject({ playing: false, position: 1 })
    src.play()
    vi.advanceTimersByTime(300)
    src.pause()
    const n = got.length
    vi.advanceTimersByTime(30_000)
    expect(got.length).toBe(n)
    src.close()
  })

  it('loops: after the end and a pause, a reset and the welcome again', () => {
    const src = new SampleWorkbookSource({ loop: true, loopPauseMs: 1000 })
    const got = collect(src)
    vi.advanceTimersByTime(120_000)
    const resets = got.filter((e) => e.type === 'reset').length
    expect(resets).toBeGreaterThanOrEqual(1)
    const afterReset = got.slice(got.findIndex((e) => e.type === 'reset') + 1)
    expect(afterReset[0]?.type).toBe('welcome')
    // The replay log starts at the last reset.
    expect(src.snapshot()[0]?.type).toBe('welcome')
    src.close()
  })

  it('a new subscriber catches up from snapshot()', () => {
    const src = new SampleWorkbookSource({ loop: false })
    vi.advanceTimersByTime(5_000)
    expect(src.snapshot().some((e) => e.type === 'keyed_snapshot')).toBe(true)
    src.close()
  })

  it('close stops every timer and event', () => {
    const src = new SampleWorkbookSource()
    const got = collect(src)
    src.close()
    vi.advanceTimersByTime(60_000)
    expect(got).toEqual([])
    expect(vi.getTimerCount()).toBe(0)
  })

  it('speed scales the waits', () => {
    const slow = new SampleWorkbookSource({ loop: false })
    const fast = new SampleWorkbookSource({ loop: false, speed: 4 })
    vi.advanceTimersByTime(3_000)
    expect(fast.status().position).toBeGreaterThan(slow.status().position)
    slow.close()
    fast.close()
  })
})
