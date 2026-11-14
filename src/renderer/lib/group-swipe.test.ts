// @vitest-environment jsdom
//
// Home groups: the two-finger sideways swipe. The tracker (threshold, one
// step per gesture, quiet gap, up/down scrolls ignored), the next/previous
// pick, where a swipe may start, and the window hook on the Homes store.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  GroupSwipeTracker,
  SWIPE_QUIET_MS,
  SWIPE_THRESHOLD_PX,
  steppedGroupId,
  swipeAllowedFrom,
} from './group-swipe'
import { useGroupSwipe, useGroupSwipeStore } from '@/hooks/useGroupSwipe'
import { selectedHome, useHomesStore } from '@/stores/homes'

describe('GroupSwipeTracker', () => {
  it('reports progress toward the threshold, then steps once (next for a leftward swipe)', () => {
    const t = new GroupSwipeTracker()
    let a = t.feed({ deltaX: SWIPE_THRESHOLD_PX / 4, deltaY: 0, time: 0 })
    expect(a).toEqual({ swipe: { progress: 0.25, direction: 1 }, step: 0 })
    a = t.feed({ deltaX: SWIPE_THRESHOLD_PX / 4, deltaY: 0, time: 16 })
    expect(a.swipe.progress).toBe(0.5)
    a = t.feed({ deltaX: SWIPE_THRESHOLD_PX / 2, deltaY: 0, time: 32 })
    expect(a).toEqual({ swipe: { progress: 0, direction: 0 }, step: 1 })
  })

  it('one gesture moves one group: the momentum tail after a step never steps again', () => {
    const t = new GroupSwipeTracker()
    const steps: number[] = []
    // 60 frames of a strong swipe and its tail, 16 ms apart (no quiet gap).
    for (let i = 0; i < 60; i++) steps.push(t.feed({ deltaX: 40, deltaY: 0, time: i * 16 }).step)
    expect(steps.filter((s) => s !== 0)).toEqual([1])
    // After a quiet gap, a new gesture steps again (the other way).
    const later = 60 * 16 + SWIPE_QUIET_MS + 1
    const back = [0, 1, 2, 3].map((i) => t.feed({ deltaX: -50, deltaY: 0, time: later + i * 16 }).step)
    expect(back.filter((s) => s !== 0)).toEqual([-1])
  })

  it('below the threshold nothing steps, and a quiet gap starts over', () => {
    const t = new GroupSwipeTracker()
    expect(t.feed({ deltaX: SWIPE_THRESHOLD_PX - 1, deltaY: 0, time: 0 }).step).toBe(0)
    expect(t.settle(SWIPE_QUIET_MS + 1)).toEqual({ progress: 0, direction: 0 })
    // The old travel is gone: one more pixel is not a step.
    expect(t.feed({ deltaX: 1, deltaY: 0, time: SWIPE_QUIET_MS + 2 }).step).toBe(0)
  })

  it('an up/down scroll is not a swipe, even with a little sideways drift', () => {
    const t = new GroupSwipeTracker()
    for (let i = 0; i < 40; i++) expect(t.feed({ deltaX: 20, deltaY: 60, time: i * 16 }).step).toBe(0)
  })

  it('line-mode deltas count in pixels', () => {
    const t = new GroupSwipeTracker()
    expect(t.feed({ deltaX: Math.ceil(SWIPE_THRESHOLD_PX / 16), deltaY: 0, deltaMode: 1, time: 0 }).step).toBe(1)
  })
})

describe('steppedGroupId', () => {
  it('moves one along and stops at either end (no wrap)', () => {
    const ids = ['a', 'b', 'c']
    expect(steppedGroupId(ids, 'a', 1)).toBe('b')
    expect(steppedGroupId(ids, 'b', -1)).toBe('a')
    expect(steppedGroupId(ids, 'c', 1)).toBeNull()
    expect(steppedGroupId(ids, 'a', -1)).toBeNull()
    expect(steppedGroupId(ids, 'gone', 1)).toBeNull()
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
  let now = 0
  let reduce = false
  beforeEach(() => {
    localStorage.clear()
    sessionStorage.clear()
    now = 1000
    reduce = false
    vi.spyOn(performance, 'now').mockImplementation(() => now)
    window.matchMedia = ((q: string) => ({ matches: reduce && q.includes('reduce') })) as unknown as typeof window.matchMedia
    const st = useHomesStore.getState()
    const before = st.homes.map((h) => h.id)
    for (const n of ['One', 'Two', 'Three']) if (st.createHome(n) === null) throw new Error(`createHome refused ${n}`)
    for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error('deleteHome refused')
    act(() => useHomesStore.getState().selectHome(useHomesStore.getState().homes[0].id))
  })
  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
  })

  const wheel = (deltaX: number, target: EventTarget = document.body, extra: WheelEventInit = {}): void => {
    now += 16
    act(() => {
      target.dispatchEvent(new WheelEvent('wheel', { deltaX, deltaY: 0, bubbles: true, ...extra }))
    })
  }
  const current = (): string => selectedHome(useHomesStore.getState()).name

  it('a swipe moves one group and the visual follows the finger', () => {
    renderHook(() => useGroupSwipe(true))
    wheel(SWIPE_THRESHOLD_PX / 2)
    expect(useGroupSwipeStore.getState().swipe).toEqual({ progress: 0.5, direction: 1 })
    for (let i = 0; i < 20; i++) wheel(40)
    expect(current()).toBe('Two')
    now += SWIPE_QUIET_MS + 1
    for (let i = 0; i < 5; i++) wheel(-40)
    expect(current()).toBe('One')
  })

  it('reduced motion: the step happens, the visual stays at rest', () => {
    reduce = true
    renderHook(() => useGroupSwipe(true))
    wheel(SWIPE_THRESHOLD_PX / 2)
    expect(useGroupSwipeStore.getState().swipe).toEqual({ progress: 0, direction: 0 })
    wheel(SWIPE_THRESHOLD_PX)
    expect(current()).toBe('Two')
  })

  it('ignored when off Home, on a pinch, when already handled, or inside a sideways scroller', () => {
    const { rerender } = renderHook(({ on }) => useGroupSwipe(on), { initialProps: { on: false } })
    wheel(SWIPE_THRESHOLD_PX * 2)
    expect(current()).toBe('One')
    rerender({ on: true })
    wheel(SWIPE_THRESHOLD_PX * 2, document.body, { ctrlKey: true })
    expect(current()).toBe('One')
    const strip = document.createElement('div')
    sideways(strip, true)
    document.body.appendChild(strip)
    wheel(SWIPE_THRESHOLD_PX * 2, strip)
    expect(current()).toBe('One')
    // Something under the pointer (a terminal) handled the wheel itself.
    // (Not on <body>: wheel listeners there are passive by default.)
    const term = document.createElement('div')
    term.addEventListener('wheel', (e) => e.preventDefault(), { passive: false })
    document.body.appendChild(term)
    wheel(SWIPE_THRESHOLD_PX * 2, term, { cancelable: true })
    expect(current()).toBe('One')
    // The same swipe on plain page does move.
    now += SWIPE_QUIET_MS + 1
    wheel(SWIPE_THRESHOLD_PX * 2)
    expect(current()).toBe('Two')
    strip.remove()
    term.remove()
  })
})
