// @vitest-environment jsdom
// The layer behind Thread view's floating objects: Glass merges the name's
// object into a neighbour, but never redraws its masks while the row moves
// (a window resize): each piece shows its own glass until it's still, and
// with no canvas they keep their own glass for good. Terminal joins them
// into one bar and follows every frame (an SVG path, no masks).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'

const masks = vi.hoisted(() => ({ result: { fill: 'data:fill', edge: 'data:edge', sheen: 'data:sheen', shadow: null } as unknown }))
vi.mock('./floatMerge', async (orig) => ({
  ...(await orig<typeof import('./floatMerge')>()),
  renderMergeMasks: vi.fn(() => masks.result),
}))

import { renderMergeMasks, MERGE_SETTLE_MS } from './floatMerge'
import { FLOAT_JOINED_ATTR, FLOAT_MERGED_ATTR, FLOAT_PIECE_ATTR, ThreadFloatMerge } from './ThreadFloatMerge'

type R = { x: number; y: number; w: number; h: number }
let rects: Record<string, R> = {}
let observers: Array<() => void> = []

function Row() {
  return (
    <div data-testid="row" data-thread-floats-grid="">
      <ThreadFloatMerge />
      <div className="thread-float-group">
        <button type="button" data-testid="far" data-k2-float="round" data-material="frost" />
        <button type="button" data-testid="near" data-k2-float="round" data-material="frost" />
      </div>
      <div>
        <div data-testid="plate" data-k2-float="name" data-material="frost" />
      </div>
    </div>
  )
}

const frame = (): void => {
  act(() => {
    vi.advanceTimersByTime(16)
  })
}
const resize = (): void => {
  act(() => {
    for (const fire of observers) fire()
  })
  frame()
}
const row = () => screen.getByTestId('row')

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['requestAnimationFrame', 'cancelAnimationFrame', 'setTimeout', 'clearTimeout'] })
  vi.mocked(renderMergeMasks).mockClear()
  masks.result = { fill: 'data:fill', edge: 'data:edge', sheen: 'data:sheen', shadow: null }
  document.documentElement.setAttribute('data-style', 'glass')
  observers = []
  // A wide row: the name's object (y 36–72) runs under `near` (13px off its
  // end, one row up) and nowhere near `far`.
  rects = {
    row: { x: 0, y: 0, w: 800, h: 80 },
    far: { x: 12, y: 10, w: 31, h: 31 },
    near: { x: 150, y: 10, w: 31, h: 31 },
    plate: { x: 194, y: 36, w: 300, h: 36 },
  }
  vi.stubGlobal(
    'ResizeObserver',
    class {
      cb: () => void
      constructor(cb: () => void) {
        this.cb = cb
        observers.push(() => this.cb())
      }
      observe() {}
      disconnect() {}
    },
  )
  vi.spyOn(Element.prototype, 'getBoundingClientRect').mockImplementation(function (this: Element) {
    const r = rects[(this as HTMLElement).dataset?.testid ?? '']
    return r ? new DOMRect(r.x, r.y, r.w, r.h) : new DOMRect(0, 0, 0, 0)
  })
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.restoreAllMocks()
  vi.unstubAllGlobals()
  resetAppZoomForTest()
  document.documentElement.removeAttribute('data-style')
})

describe('Glass', () => {
  it('merges the name with the neighbour within 16px on first mount, and only that one', () => {
    render(<Row />)
    frame()
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(true)
    expect(screen.getByTestId('plate').hasAttribute(FLOAT_PIECE_ATTR)).toBe(true)
    expect(screen.getByTestId('near').hasAttribute(FLOAT_PIECE_ATTR)).toBe(true)
    expect(screen.getByTestId('far').hasAttribute(FLOAT_PIECE_ATTR)).toBe(false)
    expect(renderMergeMasks).toHaveBeenCalledTimes(1)
    const fill = document.querySelector<HTMLElement>('[data-k2-merge="fill"]')!
    expect(fill.style.getPropertyValue('mask-image')).toContain('data:fill')
    // No drop asked for in this look: that layer isn't drawn.
    expect(document.querySelector<HTMLElement>('[data-k2-merge="shadow"]')!.hidden).toBe(true)
  })

  it('a moving row shows each piece’s own glass, and draws the masks once it has been still', async () => {
    render(<Row />)
    frame()
    expect(renderMergeMasks).toHaveBeenCalledTimes(1)
    // A window resize: every frame the row is a little narrower.
    for (let n = 1; n <= 10; n++) {
      rects.plate = { ...rects.plate, x: 194 - n, w: 300 + 2 * n }
      resize()
      expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
      expect(screen.getByTestId('near').hasAttribute(FLOAT_PIECE_ATTR)).toBe(false)
    }
    // Not one mask drawn during the drag.
    expect(renderMergeMasks).toHaveBeenCalledTimes(1)
    act(() => {
      vi.advanceTimersByTime(MERGE_SETTLE_MS - 20)
    })
    expect(renderMergeMasks).toHaveBeenCalledTimes(1)
    act(() => {
      vi.advanceTimersByTime(20)
    })
    expect(renderMergeMasks).toHaveBeenCalledTimes(2)
    // Shown once its masks are decoded (no decoder here: the next tick).
    await act(async () => {})
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(true)
  })

  describe('a redraw never shows the glass unmasked', () => {
    // A data URL set straight onto mask-image is decoded later in WebKit,
    // and the layer shows unmasked meanwhile. Here decoding waits until the
    // test lets it finish.
    let pending: Array<() => void> = []
    beforeEach(() => {
      pending = []
      vi.stubGlobal(
        'Image',
        class {
          src = ''
          decode(): Promise<void> {
            return new Promise<void>((done) => pending.push(done))
          }
        },
      )
    })
    const decodeAll = async (): Promise<void> => {
      const now = pending
      pending = []
      await act(async () => {
        for (const done of now) done()
      })
    }
    const fill = (): string => document.querySelector<HTMLElement>('[data-k2-merge="fill"]')!.style.getPropertyValue('mask-image')
    const moveAndSettle = (dx: number): void => {
      rects.plate = { ...rects.plate, x: rects.plate.x - dx, w: rects.plate.w + dx }
      resize()
      act(() => {
        vi.advanceTimersByTime(MERGE_SETTLE_MS)
      })
    }

    it('the first draw shows at once; a later one only once its masks are decoded', async () => {
      render(<Row />)
      frame()
      expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(true)
      expect(pending).toHaveLength(0)
      masks.result = { fill: 'data:fill2', edge: 'data:edge2', sheen: 'data:sheen2', shadow: null }
      moveAndSettle(2)
      expect(renderMergeMasks).toHaveBeenCalledTimes(2)
      // Drawn, not shown: each piece keeps its own glass meanwhile.
      expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
      expect(screen.getByTestId('near').hasAttribute(FLOAT_PIECE_ATTR)).toBe(false)
      expect(pending.length).toBeGreaterThan(0)
      await decodeAll()
      expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(true)
      expect(fill()).toContain('data:fill2')
    })

    it('masks decoded after the row moved on are dropped', async () => {
      render(<Row />)
      frame()
      masks.result = { fill: 'data:stale', edge: 'data:stale', sheen: 'data:stale', shadow: null }
      moveAndSettle(2)
      const stale = pending
      pending = []
      // It moves again before those are decoded.
      masks.result = { fill: 'data:fresh', edge: 'data:fresh', sheen: 'data:fresh', shadow: null }
      moveAndSettle(2)
      await act(async () => {
        for (const done of stale) done()
      })
      expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
      await decodeAll()
      expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(true)
      expect(fill()).toContain('data:fresh')
    })

    it('nothing lands after it unmounts', async () => {
      const { unmount } = render(<Row />)
      frame()
      moveAndSettle(2)
      const r = row()
      unmount()
      await decodeAll()
      expect(r.hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
    })
  })

  it('a frame where nothing moved changes nothing', () => {
    render(<Row />)
    frame()
    resize()
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(true)
    expect(renderMergeMasks).toHaveBeenCalledTimes(1)
  })

  it('apart, each keeps its own glass and no mask is drawn', () => {
    rects.plate = { ...rects.plate, x: 300, w: 200 }
    render(<Row />)
    frame()
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
    expect(renderMergeMasks).not.toHaveBeenCalled()
  })

  it('with no canvas, each piece keeps its own glass', () => {
    masks.result = null
    render(<Row />)
    frame()
    expect(renderMergeMasks).toHaveBeenCalledTimes(1)
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
    expect(document.querySelectorAll(`[${FLOAT_PIECE_ATTR}]`)).toHaveLength(0)
  })

  it('in the solid fallback (reduced transparency), nothing merges', () => {
    document.documentElement.setAttribute('data-frost', 'tile')
    render(<Row />)
    frame()
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
    expect(renderMergeMasks).not.toHaveBeenCalled()
    document.documentElement.removeAttribute('data-frost')
  })

  it('measures through the app zoom: rects are window px, the layer is CSS px', () => {
    // Zoom the way App.tsx does, so a rename of the window global can't
    // leave this test zooming nothing.
    applyAppZoomForTest(1.75)
    const z = (r: R): R => ({ x: r.x * 1.75, y: r.y * 1.75, w: r.w * 1.75, h: r.h * 1.75 })
    rects = Object.fromEntries(Object.entries(rects).map(([k, r]) => [k, z(r)]))
    render(<Row />)
    frame()
    const call = vi.mocked(renderMergeMasks).mock.calls[0][0]
    expect(call.plate.hy).toBeCloseTo(18)
    expect(call.scale).toBeCloseTo(1.75 * (window.devicePixelRatio || 1))
  })

  it('lets go of everything when it unmounts', () => {
    const { unmount } = render(<Row />)
    frame()
    const r = row()
    unmount()
    expect(r.hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
  })
})

describe('Terminal', () => {
  beforeEach(() => {
    document.documentElement.setAttribute('data-style', 'terminal')
    // The name's object starts on the objects' bottom hairline.
    rects.near = { x: 150, y: 12, w: 28, h: 28 }
    rects.far = { x: 12, y: 12, w: 28, h: 28 }
    rects.plate = { x: 192, y: 39, w: 300, h: 37 }
  })

  it('joins into one bar, with no masks, and follows a resize every frame', () => {
    render(<Row />)
    frame()
    expect(row().hasAttribute(FLOAT_JOINED_ATTR)).toBe(true)
    expect(row().hasAttribute(FLOAT_MERGED_ATTR)).toBe(false)
    expect(screen.getByTestId('near').hasAttribute(FLOAT_PIECE_ATTR)).toBe(true)
    expect(renderMergeMasks).not.toHaveBeenCalled()
    const fill = document.querySelector('[data-k2-merge="bar-fill"]')!
    const first = fill.getAttribute('d')
    expect(first).toMatch(/^M/)
    // Pulled 20px further away: apart again on the very next frame.
    rects.plate = { ...rects.plate, x: 220 }
    resize()
    expect(row().hasAttribute(FLOAT_JOINED_ATTR)).toBe(false)
    expect(screen.getByTestId('near').hasAttribute(FLOAT_PIECE_ATTR)).toBe(false)
    // And back: joined at once.
    rects.plate = { ...rects.plate, x: 186 }
    resize()
    expect(row().hasAttribute(FLOAT_JOINED_ATTR)).toBe(true)
    expect(fill.getAttribute('d')).not.toBe(first)
  })
})
