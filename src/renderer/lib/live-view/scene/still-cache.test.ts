// The stills a picture source keeps: a small LRU (about 8), so a long
// session of steps and views can't grow a page past what the browser
// allows (the shots' long pages crashed their renderer at 48 stills of
// ~15 MB each). An evicted still's bitmap is closed, so its memory goes
// at once rather than whenever the collector gets to it.

import { describe, expect, it, vi } from 'vitest'
import { STILL_CACHE_MAX, StillCache } from './still-cache'
import { StandInStills } from './stand-in/stand-in-stills'
import { DOCS } from './fold'
import type { Still } from './picture-source'

const bitmapStill = (n: number): Still & { close: ReturnType<typeof vi.fn> } => {
  const close = vi.fn()
  return { rev: n, view: '@scene:main', shape: 'wide', width: 4, height: 2, image: { kind: 'bitmap', image: { close } as unknown as CanvasImageSource }, camera: null, ids: null, renderMs: null, close }
}

describe('the still cache', () => {
  it('holds about 8 stills', () => {
    expect(STILL_CACHE_MAX).toBe(8)
  })

  it('never holds more than the cap', () => {
    const c = new StillCache()
    for (let i = 0; i < 30; i++) {
      c.set(`k${i}`, bitmapStill(i))
      expect(c.size).toBeLessThanOrEqual(STILL_CACHE_MAX)
    }
    expect(c.size).toBe(STILL_CACHE_MAX)
  })

  it('lets the least recently used still go first: a still just looked at stays', () => {
    const c = new StillCache(3)
    c.set('a', bitmapStill(1))
    c.set('b', bitmapStill(2))
    c.set('c', bitmapStill(3))
    expect(c.get('a')).toBeTruthy() // a is now the most recent
    c.set('d', bitmapStill(4))
    expect(c.keys()).toEqual(['c', 'a', 'd'])
    expect(c.get('b')).toBeUndefined()
  })

  it('closes an evicted still’s bitmap, and every bitmap when cleared', () => {
    const c = new StillCache(2)
    const [a, b, d] = [bitmapStill(1), bitmapStill(2), bitmapStill(3)]
    c.set('a', a)
    c.set('b', b)
    c.set('d', d)
    expect(a.close).toHaveBeenCalledTimes(1)
    expect(b.close).not.toHaveBeenCalled()
    c.clear()
    expect(b.close).toHaveBeenCalledTimes(1)
    expect(d.close).toHaveBeenCalledTimes(1)
    expect(c.size).toBe(0)
  })

  it('replacing a key closes the old still’s bitmap', () => {
    const c = new StillCache(2)
    const [a, a2] = [bitmapStill(1), bitmapStill(2)]
    c.set('a', a)
    c.set('a', a2)
    expect(a.close).toHaveBeenCalledTimes(1)
    expect(c.size).toBe(1)
  })
})

describe('the stand-in’s stills', () => {
  it('stay within the cap however many views are drawn', () => {
    const src = new StandInStills((rev) => DOCS[rev] ?? null)
    const views = ['@scene:main', '@cam:1', '@cam:26', '@obj:10', '@obj:15', '@turntable:0', '@turntable:1', '@turntable:2', '@turntable:3', '@turntable:4']
    for (const rev of [60, 61]) for (const view of views) src.draw({ rev, view, shape: 'wide', width: 64 })
    expect(src.cached).toBe(STILL_CACHE_MAX)
  })
})
