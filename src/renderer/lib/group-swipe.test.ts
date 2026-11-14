// @vitest-environment jsdom
//
// Home groups: the two-finger sideways swipe. The tracker (progress over
// the distance, commit past 35% on release or at once at 100%, cancel
// short of it, one step per gesture, up/down scrolls ignored), the
// next/previous pick, where a swipe may start, and the window hook on the
// Homes store.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import {
  GroupSwipeTracker,
  SWIPE_COMMIT_AT,
  SWIPE_DISTANCE_PX,
  SWIPE_QUIET_MS,
  steppedGroupId,
  swipeAllowedFrom,
} from './group-swipe'
import { useGroupSwipe, useGroupSwipeStore } from '@/hooks/useGroupSwipe'
import { selectedHome, useHomesStore } from '@/stores/homes'

const D = SWIPE_DISTANCE_PX

describe('GroupSwipeTracker', () => {
  it('reports progress and direction while the fingers move (leftward = next)', () => {
    const t = new GroupSwipeTracker()
    expect(t.feed({ deltaX: D / 4, deltaY: 0, time: 0 })).toEqual({ swipe: { progress: 0.25, direction: 'next' }, release: null })
    expect(t.feed({ deltaX: -D / 2, deltaY: 0, time: 16 })).toEqual({ swipe: { progress: 0.25, direction: 'prev' }, release: null })
  })

  it('on release (the stream goes quiet) it commits past 35% and cancels short of it', () => {
    expect(SWIPE_COMMIT_AT).toBe(0.35)
    const past = new GroupSwipeTracker()
    past.feed({ deltaX: D * 0.4, deltaY: 0, time: 0 })
    expect(past.settle(SWIPE_QUIET_MS - 1)).toEqual({ swipe: { progress: 0.4, direction: 'next' }, release: null })
    expect(past.settle(SWIPE_QUIET_MS + 1)).toEqual({ swipe: null, release: { outcome: 'commit', direction: 'next' } })
    const short = new GroupSwipeTracker()
    short.feed({ deltaX: -D * 0.3, deltaY: 0, time: 0 })
    expect(short.settle(SWIPE_QUIET_MS + 1)).toEqual({ swipe: null, release: { outcome: 'cancel', direction: 'prev' } })
    // Nothing moving: no release.
    expect(short.settle(SWIPE_QUIET_MS * 3)).toEqual({ swipe: null, release: null })
  })

  it('a full swipe commits at once; its momentum tail never steps again', () => {
    const t = new GroupSwipeTracker()
    const releases = []
    for (let i = 0; i < 60; i++) {
      const a = t.feed({ deltaX: 40, deltaY: 0, time: i * 16 })
      if (a.release) releases.push(a.release)
    }
    const end = t.settle(60 * 16 + SWIPE_QUIET_MS + 1)
    if (end.release) releases.push(end.release)
    expect(releases).toEqual([{ outcome: 'commit', direction: 'next' }])
  })

  it('a new gesture after a quiet gap releases the old one first', () => {
    const t = new GroupSwipeTracker()
    t.feed({ deltaX: D * 0.5, deltaY: 0, time: 0 })
    const a = t.feed({ deltaX: -10, deltaY: 0, time: SWIPE_QUIET_MS + 50 })
    expect(a.release).toEqual({ outcome: 'commit', direction: 'next' })
    expect(a.swipe).toEqual({ progress: 10 / D, direction: 'prev' })
  })

  it('an up/down scroll is not a swipe, even with a little sideways drift; line deltas count in pixels', () => {
    const t = new GroupSwipeTracker()
    for (let i = 0; i < 40; i++) expect(t.feed({ deltaX: 20, deltaY: 60, time: i * 16 })).toEqual({ swipe: null, release: null })
    const lines = new GroupSwipeTracker()
    expect(lines.feed({ deltaX: Math.ceil(D / 16), deltaY: 0, deltaMode: 1, time: 0 }).release).toEqual({ outcome: 'commit', direction: 'next' })
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
  let now = 0
  let nav: HTMLElement
  beforeEach(() => {
    vi.useFakeTimers()
    localStorage.clear()
    sessionStorage.clear()
    now = 1000
    vi.spyOn(performance, 'now').mockImplementation(() => now)
    useGroupSwipeStore.setState({ swipe: null, release: null })
    const st = useHomesStore.getState()
    const before = st.homes.map((h) => h.id)
    for (const n of ['One', 'Two', 'Three']) if (st.createHome(n) === null) throw new Error(`createHome refused ${n}`)
    for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error('deleteHome refused')
    act(() => useHomesStore.getState().selectHome(useHomesStore.getState().homes[0].id))
    nav = document.createElement('nav')
    nav.setAttribute('data-group-swipe-zone', '')
    document.body.appendChild(nav)
  })
  afterEach(() => {
    cleanup()
    nav.remove()
    vi.restoreAllMocks()
    vi.useRealTimers()
  })

  const wheel = (deltaX: number, target: EventTarget = nav, extra: WheelEventInit = {}): void => {
    now += 16
    act(() => {
      target.dispatchEvent(new WheelEvent('wheel', { deltaX, deltaY: 0, bubbles: true, ...extra }))
    })
  }
  const letGo = (): void => {
    now += SWIPE_QUIET_MS + 30
    act(() => {
      vi.advanceTimersByTime(SWIPE_QUIET_MS + 30)
    })
  }
  const current = (): string => selectedHome(useHomesStore.getState()).name

  it('the visual follows the finger; letting go past 35% switches once, short of it springs back', () => {
    renderHook(() => useGroupSwipe(true))
    wheel(D * 0.2)
    wheel(D * 0.2)
    expect(useGroupSwipeStore.getState().swipe).toEqual({ progress: 0.4, direction: 'next' })
    expect(current()).toBe('One')
    letGo()
    expect(current()).toBe('Two')
    expect(useGroupSwipeStore.getState()).toEqual({ swipe: null, release: { outcome: 'commit', direction: 'next', seq: 1 } })
    wheel(-D * 0.2)
    letGo()
    expect(current()).toBe('Two')
    expect(useGroupSwipeStore.getState().release).toEqual({ outcome: 'cancel', direction: 'prev', seq: 2 })
  })

  it('no wrap: a commit past the last group is a cancel', () => {
    renderHook(() => useGroupSwipe(true))
    wheel(-D * 0.6)
    letGo()
    expect(current()).toBe('One')
    expect(useGroupSwipeStore.getState().release).toEqual({ outcome: 'cancel', direction: 'prev', seq: 1 })
  })

  it('ignored outside the nav, when off Home, on a pinch, when already handled, or inside a sideways scroller', () => {
    const { rerender } = renderHook(({ on }) => useGroupSwipe(on), { initialProps: { on: false } })
    wheel(D * 2)
    letGo()
    expect(current()).toBe('One')
    rerender({ on: true })
    wheel(D * 2, document.body)
    wheel(D * 2, nav, { ctrlKey: true })
    const strip = document.createElement('div')
    sideways(strip, true)
    nav.appendChild(strip)
    wheel(D * 2, strip)
    // Something under the pointer (a list) handled the wheel itself.
    const own = document.createElement('div')
    own.addEventListener('wheel', (e) => e.preventDefault(), { passive: false })
    nav.appendChild(own)
    wheel(D * 2, own, { cancelable: true })
    letGo()
    expect(current()).toBe('One')
    expect(useGroupSwipeStore.getState().release).toBeNull()
    // The same swipe on the nav itself does move.
    wheel(D * 2)
    expect(current()).toBe('Two')
  })
})
