// @vitest-environment jsdom
//
// Home groups: the two-finger sideways swipe, paged like iOS. The tracker
// (1:1 tracking, release on a quiet gap, an empty event or the momentum's
// decay; commit past the line or on a flick, else cancel; the momentum
// tail absorbed; the axis lock; the rubber band at the ends), the glide's
// length and curve, where a swipe may start, and the window hook on the
// Homes store with fake timers and synthetic wheel streams.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  GLIDE_EASE,
  GLIDE_MAX_MS,
  GLIDE_MIN_MS,
  GroupSwipeTracker,
  SWIPE_DISTANCE_PX,
  SWIPE_TUNING as T,
  glideFor,
  steppedGroupId,
  swipeAllowedFrom,
  type SwipeAnswer,
  type SwipeDecision,
  type SwipeDirection,
} from './group-swipe'
import { GLIDE_LAND_SLACK_MS, groupSwipeLive, useGroupSwipe, useGroupSwipeStore, type SwipeFrame } from '@/hooks/useGroupSwipe'
import { selectedHome, useHomesStore } from '@/stores/homes'

const W = 300
const FRAME = 16

/** A tracker on a 300 px nav with a group on both sides unless told. */
function tracker(can: Partial<Record<SwipeDirection, boolean>> = {}): GroupSwipeTracker {
  return new GroupSwipeTracker({ distance: () => W, canGo: (d) => can[d] ?? true })
}

/** Feed deltas one frame apart from `t0`; returns every answer and the last time. */
function stream(t: GroupSwipeTracker, deltas: number[], t0 = 0, dy = 0): { answers: SwipeAnswer[]; end: number } {
  const answers = deltas.map((dx, i) => t.feed({ deltaX: dx, deltaY: dy, time: t0 + i * FRAME }))
  return { answers, end: t0 + (deltas.length - 1) * FRAME }
}

const releases = (as: SwipeAnswer[]): SwipeDecision[] => as.flatMap((a) => (a.release ? [a.release] : []))
const lastOffset = (as: SwipeAnswer[]): number | null => [...as].reverse().find((a) => a.offset !== null)?.offset ?? null

/** macOS's momentum: a smooth fall from `from`, `ratio` per frame, down to 1 px. */
function momentum(from: number, ratio = 0.9): number[] {
  const out: number[] = []
  for (let d = from; d >= 1; d *= ratio) out.push(Math.round(d * 10) / 10)
  return out
}

describe('tracking', () => {
  it('follows the fingers 1:1: the offset is the travel over the width, positive towards the next group', () => {
    const t = tracker()
    const { answers } = stream(t, [6, 4, 5, 3, 2])
    expect(answers.map((a) => a.offset)).toEqual([6 / W, 10 / W, 15 / W, 18 / W, 20 / W])
    // Back the other way past the start: the previous group.
    const back = stream(t, [-10, -15], 5 * FRAME).answers
    expect(back.map((a) => a.offset)).toEqual([10 / W, -5 / W])
    expect(releases([...answers, ...back])).toEqual([])
  })

  it('a swipe across the whole width commits at once and never steps twice', () => {
    const t = tracker()
    const { answers, end } = stream(t, Array(40).fill(20))
    expect(releases(answers)).toEqual([{ outcome: 'commit', direction: 'next', from: 1, velocity: expect.any(Number) }])
    expect(t.settle(end + 1000).release).toBeNull()
  })
})

describe('the release', () => {
  it('commits past the line (33% of the width) on a slow drag, once the stream goes quiet', () => {
    expect(T.commitAt).toBeGreaterThanOrEqual(0.3)
    expect(T.commitAt).toBeLessThanOrEqual(0.35)
    const t = tracker()
    // 0.25 px/ms: no flick. 30 × 4 px = 120 px = 40%.
    const { end } = stream(t, Array(30).fill(4))
    expect(t.settle(end + T.idleMs).release).toBeNull()
    const r = t.settle(end + T.idleMs + 1).release
    expect(r).toMatchObject({ outcome: 'commit', direction: 'next', from: 0.4 })
    expect(Math.abs(r?.velocity ?? 1) * W).toBeLessThan(T.flickPxPerMs)
  })

  it('commits on a fast flick well short of the line', () => {
    const t = tracker()
    // 12 px a frame (0.75 px/ms), 60 px = 20%, then the fingers lift.
    const { end } = stream(t, [-12, -12, -12, -12, -12])
    const r = t.settle(end + T.idleMs + 1).release
    expect(r).toMatchObject({ outcome: 'commit', direction: 'prev', from: -0.2 })
    expect((r?.velocity ?? 0) * W).toBeCloseTo(-0.75)
  })

  it('cancels short of both the line and the flick', () => {
    const t = tracker()
    const { end } = stream(t, Array(15).fill(4)) // 60 px = 20% at 0.25 px/ms
    expect(t.settle(end + T.idleMs + 1).release).toMatchObject({ outcome: 'cancel', direction: 'next', from: 0.2 })
  })

  it('past the line but flung back the other way: cancels', () => {
    const t = tracker()
    const { end } = stream(t, [...Array(40).fill(4), -12, -12, -12, -12]) // 53%, then back to 37% fast
    expect(t.settle(end + T.idleMs + 1).release).toMatchObject({ outcome: 'cancel', direction: 'next' })
  })

  it('an empty event mid-swipe is the fingers lifting', () => {
    const t = tracker()
    stream(t, Array(30).fill(4))
    expect(t.feed({ deltaX: 0, deltaY: 0, time: 30 * FRAME }).release).toMatchObject({ outcome: 'commit', direction: 'next' })
  })
})

describe('the momentum tail', () => {
  it('a lift straight into momentum (no gap) releases as the deltas start to fall, and the tail never drags or switches again', () => {
    const t = tracker()
    // Fingers: 20% at 10 px a frame, then the momentum with no gap.
    const tail = momentum(10, 0.92)
    const { answers, end } = stream(t, [4, 6, 8, 10, 10, 10, ...tail])
    const rs = releases(answers)
    expect(rs).toHaveLength(1)
    expect(rs[0]).toMatchObject({ outcome: 'commit', direction: 'next' })
    // It released within the first few momentum events, not at the tail's end.
    const at = answers.findIndex((a) => a.release)
    expect(at).toBeLessThanOrEqual(6 + T.decaySteps)
    // Nothing after the release moves the strip.
    expect(answers.slice(at + 1).every((a) => a.offset === null && a.release === null)).toBe(true)
    expect(t.settle(end + 1000).release).toBeNull()
  })

  it('after a quiet-gap release, a tail that keeps coming within 150 ms is absorbed, even a long one', () => {
    const t = tracker()
    const { end } = stream(t, Array(30).fill(4))
    expect(t.settle(end + T.idleMs + 1).release).toMatchObject({ outcome: 'commit' })
    // The tail starts 100 ms after the last finger event and runs for 1.5 s.
    const tail = stream(t, Array(90).fill(6), end + 100).answers
    expect(tail.every((a) => a.offset === null && a.release === null)).toBe(true)
  })

  it('a fresh gesture ends the tail: a gap over 150 ms, or a turn the other way', () => {
    const gap = tracker()
    const a = stream(gap, Array(30).fill(4))
    gap.settle(a.end + T.idleMs + 1)
    const after = gap.feed({ deltaX: 8, deltaY: 0, time: a.end + T.momentumGapMs + 1 })
    expect(after.offset).toBe(8 / W)

    const turn = tracker()
    const b = stream(turn, Array(30).fill(4))
    turn.settle(b.end + T.idleMs + 1)
    expect(turn.feed({ deltaX: 3, deltaY: 0, time: b.end + 90 }).offset).toBeNull()
    expect(turn.feed({ deltaX: -8, deltaY: 0, time: b.end + 106 }).offset).toBe(-8 / W)
  })
})

describe('a new swipe never waits on the tail (or on the pointer)', () => {
  it('fingers speeding up again in the tail are a fresh gesture, the same way; the falling tail never is', () => {
    const t = tracker()
    const { answers, end } = stream(t, [6, 10, 14, 14, ...momentum(14, 0.9).slice(0, 14)])
    expect(releases(answers)).toHaveLength(1)
    // New fingers, no gap: 1, 3, 6 px... the second rise starts a gesture.
    const again = stream(t, [1, 3, 6, 10, 10], end + FRAME).answers
    expect(again.slice(0, 2).every((a) => a.offset === null)).toBe(true)
    expect(again[2].offset).toBe(6 / W)
    expect(lastOffset(again)).toBe(26 / W)
  })
})

describe('the axis lock', () => {
  it('a gesture that starts up/down stays a scroll, however sideways it turns', () => {
    const t = tracker()
    const { answers, end } = stream(t, [1, 0, 2, 30, 30, 30], 0, 8)
    // dy 8 with dx 1: vertical. The later sideways deltas still don't swipe.
    expect(answers.every((a) => a.offset === null)).toBe(true)
    expect(t.settle(end + T.idleMs + 1).release).toBeNull()
  })

  it('a sideways gesture ignores the up/down drift in it', () => {
    const t = tracker()
    const { answers } = stream(t, [8, 8, 8], 0, 5)
    expect(answers.map((a) => a.offset)).toEqual([8 / W, 16 / W, 24 / W])
  })

  it('locks only after a few pixels, then counts them all', () => {
    const t = tracker()
    const { answers } = stream(t, [2, 2, 2, 2])
    expect(answers.map((a) => a.offset)).toEqual([null, null, 6 / W, 8 / W])
  })
})

describe('tracking (what the page skips its per-event checks on)', () => {
  it('true while a sideways gesture is tracked; not at rest, not up/down, not in the momentum tail', () => {
    const t = tracker()
    expect(t.tracking).toBe(false)
    const { end } = stream(t, [4, 4, 4])
    expect(t.tracking).toBe(true)
    t.settle(end + T.idleMs + 1)
    expect(t.tracking).toBe(false) // the tail
    const v = tracker()
    stream(v, [0, 0, 0], 0, 5)
    expect(v.tracking).toBe(false)
  })
})

describe('the rubber band', () => {
  it('past the last group the strip moves a third as far, and always springs back', () => {
    const t = tracker({ next: false })
    const { answers, end } = stream(t, Array(30).fill(4))
    expect(lastOffset(answers)).toBeCloseTo(0.4 * T.rubberBand)
    const r = t.settle(end + T.idleMs + 1).release
    expect(r).toMatchObject({ outcome: 'cancel', direction: 'next' })
    expect(r?.from).toBeCloseTo(0.4 / 3)
    // A flick there too.
    const f = tracker({ prev: false })
    const fl = stream(f, [-12, -12, -12, -12, -12])
    expect(f.settle(fl.end + T.idleMs + 1).release).toMatchObject({ outcome: 'cancel', direction: 'prev' })
    // It never runs away: at most a third of the width.
    const far = tracker({ next: false })
    expect(lastOffset(stream(far, Array(80).fill(10)).answers)).toBeCloseTo(T.rubberBand)
  })
})

describe('the glide', () => {
  it('goes the rest of the way on an ease-out with no bounce, 180–280 ms by the distance left', () => {
    expect([GLIDE_MIN_MS, GLIDE_MAX_MS]).toEqual([180, 280])
    const commit = glideFor({ outcome: 'commit', direction: 'next', from: 0.4, velocity: 0 })
    expect(commit).toEqual({ to: 1, ms: 240, easing: GLIDE_EASE })
    expect(GLIDE_EASE).toBe('cubic-bezier(0.25, 1, 0.5, 1)')
    expect(glideFor({ outcome: 'cancel', direction: 'prev', from: -0.2, velocity: 0 })).toEqual({ to: 0, ms: 200, easing: GLIDE_EASE })
    expect(glideFor({ outcome: 'commit', direction: 'prev', from: -0.05, velocity: 0 }).ms).toBe(275)
    expect(glideFor({ outcome: 'commit', direction: 'next', from: 1, velocity: 0 }).ms).toBe(0)
  })

  it('a fast flick glides off at the fingers’ speed (no hitch)', () => {
    // 6 px/ms on a 300 px nav from 20%: the plain curve would open slower
    // (it opens at 4 × the distance left over the duration, 3.7 px/ms here).
    const v = 6 / W
    const g = glideFor({ outcome: 'commit', direction: 'next', from: 0.2, velocity: v })
    const x1 = Number(/cubic-bezier\(([\d.]+), 1, 0\.5, 1\)/.exec(g.easing)?.[1])
    // Opening speed (share/ms) = slope × distance left / duration.
    expect(((1 / x1) * 0.8) / g.ms).toBeCloseTo(v, 3)
    // Moving the other way, or slowly: the plain curve.
    expect(glideFor({ outcome: 'cancel', direction: 'next', from: 0.2, velocity: v }).easing).toBe(GLIDE_EASE)
  })
})

describe('steppedGroupId', () => {
  it('moves one along and stops at either end (no wrap)', () => {
    const ids = ['a', 'b', 'c']
    expect(steppedGroupId(ids, 'a', 'next')).toBe('b')
    expect(steppedGroupId(ids, 'b', 'prev')).toBe('a')
    expect(steppedGroupId(ids, 'c', 'next')).toBeNull()
    expect(steppedGroupId(ids, 'a', 'prev')).toBeNull()
    expect(steppedGroupId(ids, 'gone', 'next')).toBeNull()
  })
})

function sideways(el: HTMLElement, scrolls: boolean): void {
  el.style.overflowX = 'auto'
  Object.defineProperty(el, 'scrollWidth', { configurable: true, value: scrolls ? 500 : 100 })
  Object.defineProperty(el, 'clientWidth', { configurable: true, value: 100 })
}

describe('where a swipe may start', () => {
  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('not inside something that scrolls sideways, nor inside an opt-out; anywhere else yes', () => {
    document.body.innerHTML = '<div id="strip"><span id="in-strip"></span></div><div id="wide"><span id="in-wide"></span></div><div data-group-swipe="off"><b id="in-off"></b></div><p id="plain"></p>'
    sideways(document.getElementById('strip') as HTMLElement, true)
    sideways(document.getElementById('wide') as HTMLElement, false)
    expect(swipeAllowedFrom(document.getElementById('in-strip'))).toBe(false)
    expect(swipeAllowedFrom(document.getElementById('in-wide'))).toBe(true)
    expect(swipeAllowedFrom(document.getElementById('in-off'))).toBe(false)
    expect(swipeAllowedFrom(document.getElementById('plain'))).toBe(true)
  })
})

describe('useGroupSwipe on the Homes store', () => {
  const D = SWIPE_DISTANCE_PX
  let nav: HTMLElement
  let frames: SwipeFrame[]
  let offFrames: () => void
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance', 'requestAnimationFrame', 'cancelAnimationFrame'] })
    localStorage.clear()
    sessionStorage.clear()
    useGroupSwipeStore.setState({ swipe: null, release: null })
    const st = useHomesStore.getState()
    const before = st.homes.map((h) => h.id)
    for (const n of ['One', 'Two', 'Three']) if (st.createHome(n) === null) throw new Error(`createHome refused ${n}`)
    for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error('deleteHome refused')
    act(() => useHomesStore.getState().selectHome(useHomesStore.getState().homes[0].id))
    nav = document.createElement('nav')
    nav.setAttribute('data-group-swipe-zone', '')
    document.body.appendChild(nav)
    frames = []
    offFrames = groupSwipeLive.subscribe((f) => frames.push(f))
  })
  afterEach(() => {
    offFrames()
    cleanup()
    nav.remove()
    vi.useRealTimers()
  })

  /** One wheel event, a frame after the last. */
  const wheel = (deltaX: number, target: EventTarget = nav, extra: WheelEventInit = {}): void => {
    act(() => {
      vi.advanceTimersByTime(FRAME)
      target.dispatchEvent(new WheelEvent('wheel', { deltaX, deltaY: 0, bubbles: true, ...extra }))
    })
  }
  const wait = (ms: number): void => {
    act(() => {
      vi.advanceTimersByTime(ms)
    })
  }
  const current = (): string => selectedHome(useHomesStore.getState()).name

  it('tracks, releases on a quiet gap, glides, and only then switches (the accent moves on landing)', () => {
    renderHook(() => useGroupSwipe(true))
    for (let i = 0; i < 20; i++) wheel(5) // 100 px of 240 = 42%, slowly
    expect(frames.at(-1)).toEqual({ kind: 'track', offset: 100 / D })
    expect(useGroupSwipeStore.getState().swipe).toMatchObject({ direction: 'next' })
    expect(useGroupSwipeStore.getState().swipe?.progress).toBeGreaterThanOrEqual(T.commitAt)
    wait(T.idleMs + 5)
    const glide = frames.at(-1)
    expect(glide).toMatchObject({ kind: 'glide', from: 100 / D, to: 1, outcome: 'commit' })
    if (glide?.kind !== 'glide') throw new Error('no glide')
    expect(glide.ms).toBeGreaterThanOrEqual(GLIDE_MIN_MS)
    expect(glide.ms).toBeLessThanOrEqual(GLIDE_MAX_MS)
    expect(useGroupSwipeStore.getState()).toEqual({
      swipe: { progress: 1, direction: 'next', settling: 'commit' },
      release: { outcome: 'commit', direction: 'next', seq: 1 },
    })
    expect(current()).toBe('One')
    wait(glide.ms + GLIDE_LAND_SLACK_MS)
    expect(current()).toBe('Two')
    expect(useGroupSwipeStore.getState().swipe).toBeNull()
    expect(frames.at(-1)).toEqual({ kind: 'rest', landed: true })
  })

  it('a short slow swipe glides back', () => {
    renderHook(() => useGroupSwipe(true))
    for (let i = 0; i < 8; i++) wheel(4)
    wait(T.idleMs + 5)
    expect(frames.at(-1)).toMatchObject({ kind: 'glide', to: 0, outcome: 'cancel' })
    expect(useGroupSwipeStore.getState().release).toEqual({ outcome: 'cancel', direction: 'next', seq: 1 })
    wait(GLIDE_MAX_MS + GLIDE_LAND_SLACK_MS)
    expect(current()).toBe('One')
    expect(frames.at(-1)).toEqual({ kind: 'rest', landed: false })
  })

  it('a flick and its momentum switch exactly once', () => {
    renderHook(() => useGroupSwipe(true))
    for (const d of [6, 10, 14, 14, ...momentum(14, 0.93)]) wheel(d)
    wait(1000)
    expect(current()).toBe('Two')
    expect(useGroupSwipeStore.getState().release?.seq).toBe(1)
  })

  it('a second swipe right after the first lands the first, then moves on from there', () => {
    renderHook(() => useGroupSwipe(true))
    for (let i = 0; i < 20; i++) wheel(5)
    wait(T.momentumGapMs + 5) // released and gliding; quiet past the tail's gap
    expect(current()).toBe('One')
    for (let i = 0; i < 20; i++) wheel(5)
    expect(current()).toBe('Two')
    wait(1000)
    expect(current()).toBe('Three')
  })

  // Rosson (z3mbpZ): after a swipe, the next one only worked once the
  // pointer had left Home and come back. No mouseleave or mouseenter here.
  it('swipe right, idle, swipe left: back again, the pointer never leaving the nav', () => {
    renderHook(() => useGroupSwipe(true))
    for (let i = 0; i < 20; i++) wheel(5)
    wait(1000)
    expect(current()).toBe('Two')
    for (let i = 0; i < 20; i++) wheel(-5)
    wait(1000)
    expect(current()).toBe('One')
    expect(useGroupSwipeStore.getState().release?.seq).toBe(2)
  })

  it('swipe right, then left while its momentum is still arriving: back again', () => {
    renderHook(() => useGroupSwipe(true))
    for (const d of [6, 10, 14, 14, ...momentum(14, 0.93).slice(0, 20)]) wheel(d)
    // The tail is still running (and the glide may be too): the fingers turn.
    for (let i = 0; i < 20; i++) wheel(-6)
    wait(1000)
    expect(current()).toBe('One')
    expect(useGroupSwipeStore.getState().release).toMatchObject({ outcome: 'commit', direction: 'prev', seq: 2 })
  })

  it('swipe right twice moves two groups, after a pause or straight out of the first one’s momentum', () => {
    renderHook(() => useGroupSwipe(true))
    for (let i = 0; i < 20; i++) wheel(5)
    // Released and gliding, well inside the 150 ms tail window: new fingers
    // (they start small and speed up) still go straight on.
    wait(T.idleMs + 5)
    for (const d of [1, 3, 6, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8]) wheel(d)
    wait(1000)
    expect(current()).toBe('Three')
    // From the end again: back two, the second starting in the first's tail.
    for (const d of [-6, -10, -14, -14, ...momentum(14, 0.9).slice(0, 12).map((x) => -x)]) wheel(d)
    for (const d of [-1, -3, -6, -10, -12, -12, -12, -12, -12, -12, -12]) wheel(d)
    wait(1000)
    expect(current()).toBe('One')
  })

  it('no wrap: past the last group it springs back', () => {
    renderHook(() => useGroupSwipe(true))
    for (let i = 0; i < 20; i++) wheel(-5)
    expect(frames.at(-1)).toEqual({ kind: 'track', offset: (-100 / D) * T.rubberBand })
    wait(1000)
    expect(current()).toBe('One')
    expect(useGroupSwipeStore.getState().release).toEqual({ outcome: 'cancel', direction: 'prev', seq: 1 })
  })

  it('the store is written only when what React draws changes: moving, then past the line (the strip moves outside React)', () => {
    renderHook(() => useGroupSwipe(true))
    const seen: string[] = []
    const off = useGroupSwipeStore.subscribe((st) => seen.push(`${st.swipe?.direction}:${(st.swipe?.progress ?? 0) >= T.commitAt}`))
    for (let i = 0; i < 40; i++) wheel(2) // 80 px of 240: moving, then the line at 33%
    off()
    expect(frames.filter((f) => f.kind === 'track')).toHaveLength(38) // all but the 2 before the axis locks
    expect(seen).toEqual(['next:false', 'next:true'])
  })

  it('no layout reads per event: where a swipe may start is asked once per stream, never while a gesture is tracked', () => {
    renderHook(() => useGroupSwipe(true))
    const reads = vi.spyOn(HTMLElement.prototype, 'scrollWidth', 'get')
    wheel(4)
    const oneWalk = reads.mock.calls.length
    expect(oneWalk).toBeGreaterThan(0)
    for (let i = 0; i < 30; i++) wheel(12) // the swipe, fast
    for (const d of momentum(12)) wheel(d) // its momentum: the release, then the tail absorbed
    expect(reads.mock.calls.length).toBe(oneWalk)
    expect(frames.some((f) => f.kind === 'glide')).toBe(true)
    // A new stream after a quiet gap asks again.
    wait(1000)
    wheel(4)
    expect(reads.mock.calls.length).toBe(2 * oneWalk)
    reads.mockRestore()
  })

  it('ignored outside the nav, when off Home, on a pinch, when already handled, or inside a sideways scroller', () => {
    const { rerender } = renderHook(({ on }) => useGroupSwipe(on), { initialProps: { on: false } })
    for (let i = 0; i < 20; i++) wheel(20)
    wait(1000)
    expect(current()).toBe('One')
    rerender({ on: true })
    const strip = document.createElement('div')
    sideways(strip, true)
    nav.appendChild(strip)
    const own = document.createElement('div')
    own.addEventListener('wheel', (e) => e.preventDefault(), { passive: false })
    nav.appendChild(own)
    for (let i = 0; i < 20; i++) {
      wheel(20, document.body)
      wheel(20, nav, { ctrlKey: true })
      wheel(20, strip)
      wheel(20, own, { cancelable: true })
    }
    wait(1000)
    expect(current()).toBe('One')
    expect(useGroupSwipeStore.getState().release).toBeNull()
    // The same swipe on the nav itself does move.
    for (let i = 0; i < 20; i++) wheel(20)
    wait(1000)
    expect(current()).toBe('Two')
  })
})
