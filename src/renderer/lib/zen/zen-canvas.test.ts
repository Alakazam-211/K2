// @vitest-environment jsdom
//
// Zen 0.45.3: using the whole canvas — hit regions cut out of the floating
// top band's drag strip, K2's chrome wins, a minimum drag area always stays,
// and window.startDrag only during a real press in that frame.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  __resetZenCanvasForTests,
  registerZenCanvasFrame,
  setZenCanvasRegions,
  startZenCanvasDrag,
  subscribeZenCanvas,
  zenBandDragPlan,
  zenCanvasHoles,
  zenCanvasPointer,
  zenCanvasVersion,
  zenHitRegionsFrom,
  zenRectIntersect,
  zenSubtractRects,
  ZenCanvasError,
  ZEN_DRAG_PRESS_MAX_MS,
  ZEN_HIT_REGION_APPLY_MS,
  ZEN_HIT_REGION_MAX,
} from './zen-canvas'
import { ZEN_DRAG_MIN_SIZE, type ZenRect } from './zen-controls'

const R = (left: number, top: number, width: number, height: number): ZenRect => ({ left, top, width, height })

function area(rs: readonly ZenRect[]): number {
  return rs.reduce((n, r) => n + r.width * r.height, 0)
}

function inside(r: ZenRect, x: number, y: number): boolean {
  return x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height
}

/** What a click at (x, y) of the band hits, by the plan: K2's chrome (on
 *  top), a drag piece, or the frame below. Band-local coordinates. */
function hit(plan: ReturnType<typeof zenBandDragPlan>, chrome: readonly ZenRect[], x: number, y: number): 'chrome' | 'drag' | 'frame' {
  if (chrome.some((c) => inside(c, x, y))) return 'chrome'
  if (!plan) return 'drag'
  return plan.pieces.some((p) => inside(p, x, y)) ? 'drag' : 'frame'
}

// A band 800 × 52 at the window's top; K2's groups at the two ends.
const BAND = R(0, 0, 800, 52)
const START = R(14, 4, 120, 44)
const END = R(666, 4, 120, 44)
const SPACER = R(146, 0, 508, 52)
const MIN = ZEN_DRAG_MIN_SIZE

describe('hit regions: input', () => {
  it('takes {x, y, width, height} lists (DOMRect-like too); zero-area rects are dropped', () => {
    expect(zenHitRegionsFrom([])).toEqual([])
    expect(zenHitRegionsFrom([{ x: 1, y: 2, width: 3, height: 4 }, { x: 0, y: 0, width: 0, height: 9 }])).toEqual([R(1, 2, 3, 4)])
    const domRectLike = Object.create({ get x() { return 5 }, get y() { return 6 }, get width() { return 7 }, get height() { return 8 } })
    expect(zenHitRegionsFrom([domRectLike])).toEqual([R(5, 6, 7, 8)])
  })

  it('refuses anything else: not a list, a missing or non-finite number, a negative size', () => {
    const bad: unknown[] = [
      null,
      { x: 1 },
      [null],
      [{ x: 1, y: 2, width: 3 }],
      [{ x: '1', y: 2, width: 3, height: 4 }],
      [{ x: Number.NaN, y: 2, width: 3, height: 4 }],
      [{ x: 1, y: Number.POSITIVE_INFINITY, width: 3, height: 4 }],
      [{ x: 1, y: 2, width: -3, height: 4 }],
    ]
    for (const b of bad) {
      expect(() => zenHitRegionsFrom(b), JSON.stringify(b)).toThrow(ZenCanvasError)
      try {
        zenHitRegionsFrom(b)
      } catch (e) {
        expect((e as ZenCanvasError).code).toBe('failed')
      }
    }
  })

  it(`caps the list at ${ZEN_HIT_REGION_MAX} rects (too_large)`, () => {
    const one = { x: 0, y: 0, width: 1, height: 1 }
    expect(zenHitRegionsFrom(Array(ZEN_HIT_REGION_MAX).fill(one))).toHaveLength(ZEN_HIT_REGION_MAX)
    expect(() => zenHitRegionsFrom(Array(ZEN_HIT_REGION_MAX + 1).fill(one))).toThrow(
      expect.objectContaining({ code: 'too_large' }) as unknown as Error,
    )
  })
})

describe('hit regions: geometry', () => {
  it('intersect and subtract keep the area exact', () => {
    expect(zenRectIntersect(R(0, 0, 10, 10), R(5, 5, 10, 10))).toEqual(R(5, 5, 5, 5))
    expect(zenRectIntersect(R(0, 0, 10, 10), R(10, 0, 5, 5))).toBeNull()
    const base = R(0, 0, 100, 50)
    const holes = [R(10, 10, 20, 20), R(20, 0, 30, 15), R(90, 40, 50, 50)]
    const out = zenSubtractRects(base, holes)
    // Union of the holes inside the base: 400 + 450 - overlap(20..30 × 10..15 = 50) + 100 = 900.
    expect(area(out)).toBe(100 * 50 - 900)
    for (const r of out) for (const h of holes) expect(zenRectIntersect(r, h)).toBeNull()
    expect(zenSubtractRects(base, [])).toEqual([base])
    expect(zenSubtractRects(base, [R(-5, -5, 200, 200)])).toEqual([])
  })

  it('a hole cuts the strip: a click in it reaches the frame; outside it the strip still drags', () => {
    const hole = R(300, 10, 80, 32)
    const plan = zenBandDragPlan({ band: BAND, holes: [hole], chrome: [START, END], spacers: [SPACER], min: MIN })
    expect(plan).not.toBeNull()
    expect(plan!.kept).toBe(false)
    expect(hit(plan, [START, END], 340, 26)).toBe('frame')
    expect(hit(plan, [START, END], 250, 26)).toBe('drag')
    expect(hit(plan, [START, END], 340, 5)).toBe('drag') // above the hole
    expect(area(plan!.pieces)).toBe(800 * 52 - 80 * 32)
  })

  it('maps holes from window coordinates (a band lower in the window) and clamps them to the band', () => {
    const band = R(0, 30, 800, 52)
    const plan = zenBandDragPlan({ band, holes: [R(700, 0, 300, 60)], chrome: [], spacers: [], min: MIN })
    // Band-local: x 700..800, y 0..30.
    expect(hit(plan, [], 750, 10)).toBe('frame')
    expect(hit(plan, [], 750, 40)).toBe('drag')
    expect(area(plan!.pieces)).toBe(800 * 52 - 100 * 30)
  })

  it('no hole touching the band: no plan (the whole band drags, as before)', () => {
    expect(zenBandDragPlan({ band: BAND, holes: [], chrome: [START], spacers: [SPACER], min: MIN })).toBeNull()
    expect(zenBandDragPlan({ band: BAND, holes: [R(0, 100, 800, 500)], chrome: [START], spacers: [SPACER], min: MIN })).toBeNull()
  })

  it('K2 chrome wins: a hole over a control is ignored there (the control still gets the click)', () => {
    const hole = R(0, 0, 400, 52) // covers the start group
    const plan = zenBandDragPlan({ band: BAND, holes: [hole], chrome: [START, END], spacers: [SPACER], min: MIN })
    expect(hit(plan, [START, END], 50, 26)).toBe('chrome')
    expect(hit(plan, [START, END], 300, 26)).toBe('frame')
    expect(hit(plan, [START, END], 500, 26)).toBe('drag')
  })

  it('holes over the whole strip: K2 keeps the gaps between its groups as drag area', () => {
    const plan = zenBandDragPlan({ band: BAND, holes: [R(0, 0, 800, 52)], chrome: [START, END], spacers: [SPACER], min: MIN })
    expect(plan!.kept).toBe(true)
    expect(hit(plan, [START, END], 400, 26)).toBe('drag')
    expect(hit(plan, [START, END], 5, 26)).toBe('frame') // outside the groups and the gaps
    expect(hit(plan, [START, END], 700, 26)).toBe('chrome')
  })

  it('a leftover strip thinner or narrower than the drag minimum doesn’t count as drag space', () => {
    // Leaves 800 × 10 at the bottom (under the 12 px minimum) outside the groups.
    const plan = zenBandDragPlan({ band: BAND, holes: [R(0, 0, 800, 42)], chrome: [START, END], spacers: [SPACER], min: MIN })
    expect(plan!.kept).toBe(true)
    // Leaves a 130 × 52 gap: enough.
    const ok = zenBandDragPlan({ band: BAND, holes: [R(0, 0, 400, 52), R(530, 0, 270, 52)], chrome: [START, END], spacers: [SPACER], min: MIN })
    expect(ok!.kept).toBe(false)
    expect(hit(ok, [START, END], 460, 26)).toBe('drag')
  })

  it('the title strip (no groups) keeps a centred minimum when holes cover it', () => {
    const strip = R(0, 0, 800, 28)
    const plan = zenBandDragPlan({ band: strip, holes: [R(0, 0, 800, 28)], chrome: [], spacers: [], min: MIN })
    expect(plan!.kept).toBe(true)
    expect(plan!.pieces).toEqual([R(340, 0, 120, 28)])
  })
})

describe('the frames on the page', () => {
  function frameAt(rect: ZenRect): HTMLIFrameElement {
    const el = document.createElement('iframe')
    document.body.appendChild(el)
    el.getBoundingClientRect = () =>
      ({ ...rect, x: rect.left, y: rect.top, right: rect.left + rect.width, bottom: rect.top + rect.height, toJSON: () => rect }) as DOMRect
    return el
  }

  beforeEach(() => {
    vi.useFakeTimers()
    __resetZenCanvasForTests()
  })
  afterEach(() => {
    __resetZenCanvasForTests()
    document.body.innerHTML = ''
    vi.useRealTimers()
  })

  it('maps a frame’s regions to window coordinates, clamped to the frame; [] clears them', () => {
    const el = frameAt(R(100, 0, 600, 400))
    registerZenCanvasFrame('g/a', el)
    setZenCanvasRegions('g/a', [R(10, 5, 50, 20), R(580, -10, 100, 30)])
    expect(zenCanvasHoles()).toEqual([R(110, 5, 50, 20), R(680, 0, 20, 20)])
    setZenCanvasRegions('g/a', [])
    expect(zenCanvasHoles()).toEqual([])
  })

  it('regions go with their frame: unregistering forgets them, a new frame starts clean', () => {
    const el = frameAt(R(0, 0, 400, 400))
    const off = registerZenCanvasFrame('g/a', el)
    setZenCanvasRegions('g/a', [R(0, 0, 10, 10)])
    off()
    expect(zenCanvasHoles()).toEqual([])
    const a = frameAt(R(0, 0, 400, 400))
    registerZenCanvasFrame('g/b', a)
    setZenCanvasRegions('g/b', [R(0, 0, 10, 10)])
    registerZenCanvasFrame('g/b', frameAt(R(0, 0, 400, 400)))
    expect(zenCanvasHoles()).toEqual([])
  })

  it('a widget changing its regions every frame re-plans the band at most every 50 ms', () => {
    registerZenCanvasFrame('g/a', frameAt(R(0, 0, 400, 400)))
    const seen: number[] = []
    subscribeZenCanvas(() => seen.push(zenCanvasVersion()))
    vi.advanceTimersByTime(ZEN_HIT_REGION_APPLY_MS)
    seen.length = 0
    for (let i = 0; i < 60; i++) {
      setZenCanvasRegions('g/a', [R(i, 0, 10, 10)])
      vi.advanceTimersByTime(16)
    }
    vi.advanceTimersByTime(ZEN_HIT_REGION_APPLY_MS)
    // 60 changes over ~1 s: about 20 notices, never 60.
    expect(seen.length).toBeGreaterThan(0)
    expect(seen.length).toBeLessThanOrEqual(Math.ceil((60 * 16) / ZEN_HIT_REGION_APPLY_MS) + 1)
    // The last notice carries the last list.
    expect(zenCanvasHoles()).toEqual([R(59, 0, 10, 10)])
    // The same list again is no change at all.
    const before = seen.length
    setZenCanvasRegions('g/a', [R(59, 0, 10, 10)])
    vi.advanceTimersByTime(ZEN_HIT_REGION_APPLY_MS * 2)
    expect(seen.length).toBe(before)
  })
})

describe('window.startDrag', () => {
  let el: HTMLIFrameElement
  beforeEach(() => {
    __resetZenCanvasForTests()
    el = document.createElement('iframe')
    document.body.appendChild(el)
    registerZenCanvasFrame('g/a', el)
  })
  afterEach(() => {
    __resetZenCanvasForTests()
    document.body.innerHTML = ''
  })

  it('starts the native drag only while a press is down in that frame, and the frame has focus', () => {
    const drag = vi.fn()
    expect(() => startZenCanvasDrag('g/a', { now: 1000, activeElement: el, drag })).toThrow(/button down/)
    zenCanvasPointer('g/a', true, 1000)
    startZenCanvasDrag('g/a', { now: 1200, activeElement: el, drag })
    expect(drag).toHaveBeenCalledTimes(1)
  })

  it('one drag per press: a second call in the same press is refused', () => {
    const drag = vi.fn()
    zenCanvasPointer('g/a', true, 1000)
    startZenCanvasDrag('g/a', { now: 1000, activeElement: el, drag })
    expect(() => startZenCanvasDrag('g/a', { now: 1001, activeElement: el, drag })).toThrow(ZenCanvasError)
    expect(drag).toHaveBeenCalledTimes(1)
  })

  it('a released, stale or other frame’s press never drags', () => {
    const drag = vi.fn()
    zenCanvasPointer('g/a', true, 1000)
    zenCanvasPointer('g/a', false, 1100)
    expect(() => startZenCanvasDrag('g/a', { now: 1200, activeElement: el, drag })).toThrow(ZenCanvasError)
    zenCanvasPointer('g/a', true, 1000)
    expect(() => startZenCanvasDrag('g/a', { now: 1000 + ZEN_DRAG_PRESS_MAX_MS + 1, activeElement: el, drag })).toThrow(ZenCanvasError)
    zenCanvasPointer('g/b', true, 1000)
    expect(() => startZenCanvasDrag('g/a', { now: 1000, activeElement: el, drag })).toThrow(ZenCanvasError)
    expect(() => startZenCanvasDrag('g/b', { now: 1000, activeElement: el, drag })).toThrow(ZenCanvasError) // no frame
    expect(drag).not.toHaveBeenCalled()
  })

  it('needs the frame to have the focus or the pointer', () => {
    const drag = vi.fn()
    zenCanvasPointer('g/a', true, 1000)
    expect(() => startZenCanvasDrag('g/a', { now: 1000, activeElement: document.body, drag })).toThrow(/focus nor the pointer/)
    zenCanvasPointer('g/a', true, 1000)
    el.dispatchEvent(new MouseEvent('mouseenter'))
    startZenCanvasDrag('g/a', { now: 1000, activeElement: document.body, drag })
    expect(drag).toHaveBeenCalledTimes(1)
    el.dispatchEvent(new MouseEvent('mouseleave'))
    zenCanvasPointer('g/a', true, 1000)
    expect(() => startZenCanvasDrag('g/a', { now: 1000, activeElement: document.body, drag })).toThrow(ZenCanvasError)
  })
})
